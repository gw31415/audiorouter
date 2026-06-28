//! Audio engine: stream startup/shutdown, ring buffers, real-time mixing.
//!
//! Uses one SPSC ring buffer per route (SPEC.md §8.3, option 1) to support
//! fan-out (one input feeding multiple outputs). The input callback for device
//! `D` writes its full physical interleaved frames into every route buffer
//! where `D` is `from`. The output callback for device `O` reads from every
//! route buffer where `O` is `to`.
//!
//! Hot-reload: when the config file changes on disk, the engine tears down
//! only the affected streams and rebuilds them from the new plan — no process
//! restart, no TUI disruption.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    Device, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, Stream, StreamConfig,
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use crate::meter::MeterBank;
use crate::mixer::{db_to_linear, hard_limit_buffer, mix_route_interleaved};
use crate::validate::ValidatedConfig;

// ─── AudioEngine ───────────────────────────────────────────────────────────

/// The audio engine owns all streams and ring buffers for a single plan.
///
/// When the config file changes, call [`AudioEngine::reload`] to swap in a
/// new plan without restarting the process.
pub struct AudioEngine {
    plan: ValidatedConfig,
    resolved: crate::devices::ResolvedAudioDevices,
    config_path: PathBuf,
    input_streams: Option<Vec<Stream>>,
    output_streams: Option<Vec<Stream>>,
    meter_bank: Arc<MeterBank>,
    running: Arc<AtomicBool>,
    fatal_error: Arc<AtomicBool>,
}

/// Result of an engine state query — used by the TUI main loop.
pub enum EngineState {
    /// Engine is running normally.
    Running,
    /// A fatal audio error occurred.
    FatalError,
    /// Ctrl-C was received.
    Stopped,
}

impl AudioEngine {
    /// Create a new engine and open all streams immediately.
    ///
    /// # Errors
    ///
    /// Returns `AppError` (Runtime) on fatal audio/stream errors.
    pub fn new(
        plan: ValidatedConfig,
        resolved: crate::devices::ResolvedAudioDevices,
        config_path: &Path,
    ) -> Result<Self, crate::error::AppError> {
        let meter_bank = Arc::new(MeterBank::for_plan(&plan));
        let running = Arc::new(AtomicBool::new(true));
        let fatal_error = Arc::new(AtomicBool::new(false));

        let mut engine = Self {
            plan,
            resolved,
            config_path: config_path.to_path_buf(),
            input_streams: None,
            output_streams: None,
            meter_bank,
            running,
            fatal_error,
        };

        engine.open_all_streams()?;
        Ok(engine)
    }

    /// Shared handle to the meter bank (for TUI reads).
    pub fn meter_bank(&self) -> &Arc<MeterBank> {
        &self.meter_bank
    }

    /// Current validated plan.
    pub fn plan(&self) -> &ValidatedConfig {
        &self.plan
    }

    /// Current CoreAudio device resolution, including runtime-disabled routes.
    pub fn resolved(&self) -> &crate::devices::ResolvedAudioDevices {
        &self.resolved
    }

    /// Signal the engine to stop.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Check the current engine state.
    pub fn state(&self) -> EngineState {
        if self.fatal_error.load(Ordering::SeqCst) {
            EngineState::FatalError
        } else if self.running.load(Ordering::SeqCst) {
            EngineState::Running
        } else {
            EngineState::Stopped
        }
    }

    /// Hot-reload: re-read config, re-validate, re-resolve devices, and
    /// rebuild streams — all without restarting the process.
    ///
    /// Drops existing streams first (causing a brief audio gap), then opens
    /// new ones. If the new config is invalid, keeps the old streams running
    /// and returns an error.
    ///
    /// # Errors
    ///
    /// Returns `AppError` if the new config fails validation or device
    /// resolution. In that case the old engine state is preserved.
    pub fn reload(&mut self) -> Result<(), crate::error::AppError> {
        // Read and validate the new config.
        let config = crate::config::read_config(&self.config_path)
            .map_err(|e| crate::error::AppError::config(format!("{e}")))?;
        let new_plan = crate::validate::validate_config(config).map_err(|errors| {
            crate::error::AppError::config(format!(
                "config validation failed:\n{}",
                errors.join("\n")
            ))
        })?;
        let new_resolved = crate::devices::resolve_devices(&new_plan)?;
        let new_meter_bank = Arc::new(MeterBank::for_plan(&new_plan));

        // Swap everything atomically.
        self.teardown_streams();

        self.plan = new_plan;
        self.resolved = new_resolved;
        self.meter_bank = new_meter_bank;

        self.open_all_streams()?;
        Ok(())
    }

    /// Re-scan CoreAudio devices while running and rebuild streams if device
    /// connectivity changed. Returns connection/disconnection log messages.
    ///
    /// Unlike startup, this does not surface missing-device warnings. A device
    /// disappearing or reappearing during normal operation is represented only
    /// as a connectivity event in the TUI log.
    pub fn refresh_devices(&mut self) -> Result<Vec<String>, crate::error::AppError> {
        let new_resolved = crate::devices::resolve_devices(&self.plan)?;
        let events = self.resolved.connectivity_events(&new_resolved, &self.plan);
        let route_state_changed =
            self.resolved.disabled_route_indices != new_resolved.disabled_route_indices;

        if events.is_empty() && !route_state_changed {
            return Ok(Vec::new());
        }

        self.teardown_streams();
        self.resolved = new_resolved;
        self.open_all_streams()?;

        Ok(events)
    }

    /// Tear down all streams.
    fn teardown_streams(&mut self) {
        // Drop output first, then input, to minimise clicks.
        self.output_streams.take();
        self.input_streams.take();
    }

    /// Open all input and output streams for the current plan.
    fn open_all_streams(&mut self) -> Result<(), crate::error::AppError> {
        let sample_rate = self.plan.config.engine.sample_rate;

        // Set sample rate on all meters for Goertzel band analysis.
        for (_, meter) in &self.meter_bank.meters {
            meter.set_sample_rate(sample_rate);
        }

        // ─── Pre-split all per-route ring buffers ───────────────────────
        let mut route_producers: HashMap<usize, HeapProd<f32>> = HashMap::new();
        let mut route_consumers: HashMap<usize, (HeapCons<f32>, usize)> = HashMap::new();
        let mut route_input_channels: HashMap<usize, usize> = HashMap::new();
        let buffer_size = self.plan.config.engine.buffer_size as usize;

        for (i, route) in self.plan.routes.iter().enumerate() {
            if !self.resolved.route_enabled(i) {
                continue;
            }
            let from_device = &self.resolved.devices[&route.from];
            let channels = if from_device.is_input && from_device.max_input_channels > 0 {
                from_device.max_input_channels as usize
            } else {
                self.plan
                    .device_by_name(&route.from)
                    .map(|r| r.required_input_channels.max(1))
                    .unwrap_or(1)
            };

            let capacity = buffer_size * channels * 4;
            let rb = HeapRb::<f32>::new(capacity);
            let (prod, cons) = rb.split();

            route_producers.insert(i, prod);
            route_consumers.insert(i, (cons, channels));
            route_input_channels.insert(i, channels);
        }

        // ─── Open input streams ─────────────────────────────────────────
        for alias in self.resolved.input_device_names() {
            let resolved_dev = &self.resolved.devices[alias];

            let route_indices: Vec<usize> = self
                .plan
                .routes
                .iter()
                .enumerate()
                .filter(|(i, r)| self.resolved.route_enabled(*i) && r.from == alias)
                .map(|(i, _)| i)
                .collect();

            if route_indices.is_empty() {
                continue;
            }

            let channels = route_input_channels[&route_indices[0]];

            let supported = find_config_for(&resolved_dev.device, true, sample_rate, channels)
                .map_err(crate::error::AppError::runtime)?;

            let stream_config = StreamConfig {
                channels: supported.channels(),
                sample_rate,
                buffer_size: cpal::BufferSize::Default,
            };
            let sample_format = supported.sample_format();

            let mut producers: Vec<HeapProd<f32>> = Vec::new();
            for &ri in &route_indices {
                let prod = route_producers.remove(&ri).unwrap();
                producers.push(prod);
            }

            let stream = build_input_stream(
                &resolved_dev.device,
                stream_config,
                sample_format,
                producers,
                &self.fatal_error,
                alias.to_string(),
                self.meter_bank.clone(),
            )
            .map_err(|e| {
                crate::error::AppError::runtime(format!(
                    "failed to open input stream for device \"{}\": {e}",
                    resolved_dev.name
                ))
            })?;

            stream.play().map_err(|e| {
                crate::error::AppError::runtime(format!(
                    "failed to start input stream for device \"{}\": {e}",
                    resolved_dev.name
                ))
            })?;

            self.input_streams.get_or_insert_with(Vec::new).push(stream);
        }

        // ─── Open output streams ────────────────────────────────────────
        for alias in self.resolved.output_device_names() {
            let resolved_dev = &self.resolved.devices[alias];

            let route_indices: Vec<usize> = self
                .plan
                .routes
                .iter()
                .enumerate()
                .filter(|(i, r)| self.resolved.route_enabled(*i) && r.to == alias)
                .map(|(i, _)| i)
                .collect();

            if route_indices.is_empty() {
                continue;
            }

            let out_channels = resolved_dev.max_output_channels as usize;
            let supported = find_config_for(&resolved_dev.device, false, sample_rate, out_channels)
                .map_err(crate::error::AppError::runtime)?;

            let stream_config = StreamConfig {
                channels: supported.channels(),
                sample_rate,
                buffer_size: cpal::BufferSize::Default,
            };
            let sample_format = supported.sample_format();

            let mut consumers: Vec<ConsumerEntry> = Vec::new();
            let mut route_meta: Vec<RouteMixMeta> = Vec::new();
            for &ri in &route_indices {
                let (cons, ch) = route_consumers.remove(&ri).unwrap();
                consumers.push(ConsumerEntry {
                    consumer: cons,
                    channels: ch,
                });

                let route = &self.plan.routes[ri];
                route_meta.push(RouteMixMeta {
                    from_channels: route.from_channels.clone(),
                    to_channels: route.to_channels.clone(),
                    gain: if route.mute {
                        0.0
                    } else {
                        db_to_linear(route.gain_db)
                    },
                });
            }

            let limiter = self
                .plan
                .device_by_name(alias)
                .map(|d| d.limiter)
                .unwrap_or(false);

            let stream = build_output_stream(
                &resolved_dev.device,
                stream_config,
                sample_format,
                out_channels,
                consumers,
                route_meta,
                limiter,
                &self.fatal_error,
                alias.to_string(),
                self.meter_bank.clone(),
            )
            .map_err(|e| {
                crate::error::AppError::runtime(format!(
                    "failed to open output stream for device \"{}\": {e}",
                    resolved_dev.name
                ))
            })?;

            stream.play().map_err(|e| {
                crate::error::AppError::runtime(format!(
                    "failed to start output stream for device \"{}\": {e}",
                    resolved_dev.name
                ))
            })?;

            self.output_streams
                .get_or_insert_with(Vec::new)
                .push(stream);
        }

        Ok(())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.teardown_streams();
    }
}

// ─── Config watcher ────────────────────────────────────────────────────────

/// Watches the config file for changes in a background thread.
///
/// When a change is detected (after debounce), sets a shared flag that the
/// main loop can poll.
pub struct ConfigWatcher {
    config_changed: Arc<AtomicBool>,
}

impl ConfigWatcher {
    /// Start watching the config file. Returns a watcher handle.
    pub fn new(config_path: &Path) -> Self {
        let config_changed = Arc::new(AtomicBool::new(false));
        let flag = config_changed.clone();
        let watch_path = config_path.to_path_buf();

        std::thread::spawn(move || {
            use notify::{EventKind, RecursiveMode, Watcher};

            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::warn!("config watch disabled: {e}");
                    return;
                }
            };

            let canonical_watch_path = std::fs::canonicalize(&watch_path).ok();

            for watch_dir in config_watch_dirs(&watch_path, canonical_watch_path.as_deref()) {
                if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                    tracing::warn!("config watch disabled: {e}");
                    return;
                }
            }

            for event in rx.into_iter().flatten() {
                let is_config_event = config_event_matches(
                    &event.paths,
                    &watch_path,
                    canonical_watch_path.as_deref(),
                );
                if !is_config_event {
                    continue;
                }
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        flag.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });

        Self { config_changed }
    }

    /// Check (and consume) the config-changed flag.
    pub fn poll(&self) -> bool {
        self.config_changed.swap(false, Ordering::SeqCst)
    }
}

/// Metadata for each route used by the output callback mixer.
struct RouteMixMeta {
    from_channels: Vec<usize>,
    to_channels: Vec<usize>,
    gain: f32,
}

/// Consumer side of a per-route ring buffer.
struct ConsumerEntry {
    consumer: HeapCons<f32>,
    channels: usize,
}

fn find_config_for(
    device: &Device,
    is_input: bool,
    sample_rate: u32,
    desired_channels: usize,
) -> Result<cpal::SupportedStreamConfig, String> {
    let configs: Vec<cpal::SupportedStreamConfigRange> = if is_input {
        device
            .supported_input_configs()
            .map_err(|e| format!("supported configs query failed: {e}"))?
            .collect()
    } else {
        device
            .supported_output_configs()
            .map_err(|e| format!("supported configs query failed: {e}"))?
            .collect()
    };

    let mut best: Option<cpal::SupportedStreamConfigRange> = None;
    for range in &configs {
        let min = range.min_sample_rate();
        let max = range.max_sample_rate();
        if sample_rate >= min
            && sample_rate <= max
            && range.channels() >= desired_channels as u16
            && best
                .as_ref()
                .is_none_or(|b| range.channels() > b.channels())
        {
            best = Some(*range);
        }
    }

    if best.is_none() {
        for range in &configs {
            let min = range.min_sample_rate();
            let max = range.max_sample_rate();
            if sample_rate >= min && sample_rate <= max {
                best = Some(*range);
                break;
            }
        }
    }

    let range = best.ok_or_else(|| {
        format!(
            "no supported config at {} Hz with >= {} channels",
            sample_rate, desired_channels
        )
    })?;

    Ok(range.with_sample_rate(sample_rate))
}

// ─── Input stream ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_input_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    producers: Vec<HeapProd<f32>>,
    fatal_error: &Arc<AtomicBool>,
    device_alias: String,
    meter_bank: Arc<MeterBank>,
) -> Result<Stream, cpal::Error> {
    let _ = fatal_error;
    let err_fn = move |err| {
        tracing::warn!("input stream error: {err}");
    };

    let producers = Arc::new(Mutex::new(producers));
    let meter_bank_for_cb = meter_bank.clone();
    let alias_for_cb = device_alias.clone();

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &InputCallbackInfo| {
                input_callback(data, &producers);
                update_input_meters(
                    data,
                    config.channels as usize,
                    &alias_for_cb,
                    &meter_bank_for_cb,
                );
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &InputCallbackInfo| {
                input_callback(data, &producers);
                update_input_meters_i(
                    data,
                    config.channels as usize,
                    &alias_for_cb,
                    &meter_bank_for_cb,
                );
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &InputCallbackInfo| {
                input_callback(data, &producers);
                update_input_meters_i(
                    data,
                    config.channels as usize,
                    &alias_for_cb,
                    &meter_bank_for_cb,
                );
            },
            err_fn,
            None,
        )?,
        SampleFormat::I32 => device.build_input_stream(
            config,
            move |data: &[i32], _: &InputCallbackInfo| {
                input_callback(data, &producers);
                update_input_meters_i(
                    data,
                    config.channels as usize,
                    &alias_for_cb,
                    &meter_bank_for_cb,
                );
            },
            err_fn,
            None,
        )?,
        _ => return Err(cpal::Error::new(cpal::ErrorKind::UnsupportedConfig)),
    };

    Ok(stream)
}

fn input_callback<T: ToF32>(data: &[T], producers: &Arc<Mutex<Vec<HeapProd<f32>>>>) {
    if data.is_empty() {
        return;
    }
    let samples: Vec<f32> = data.iter().map(|s| s.to_f32()).collect();

    let mut guard = match producers.lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    for prod in guard.iter_mut() {
        prod.push_slice(&samples);
    }
}

/// Update per-channel meters from interleaved input data.
fn update_input_meters(data: &[f32], channels: usize, alias: &str, meter_bank: &MeterBank) {
    if channels == 0 {
        return;
    }
    let frames = data.len() / channels;
    for ch in 1..=channels {
        let Some(meter) = meter_bank.get(alias, ch) else {
            continue;
        };
        let ch_samples: Vec<f32> = (0..frames).map(|f| data[f * channels + (ch - 1)]).collect();
        meter.update(&ch_samples);
    }
}

/// Update per-channel meters from interleaved integer input data.
fn update_input_meters_i<T: ToF32>(
    data: &[T],
    channels: usize,
    alias: &str,
    meter_bank: &MeterBank,
) {
    if channels == 0 {
        return;
    }
    let f32_data: Vec<f32> = data.iter().map(|s| s.to_f32()).collect();
    update_input_meters(&f32_data, channels, alias, meter_bank);
}

// ─── Output stream ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_output_stream(
    device: &Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    out_channels: usize,
    consumers: Vec<ConsumerEntry>,
    route_meta: Vec<RouteMixMeta>,
    limiter: bool,
    fatal_error: &Arc<AtomicBool>,
    device_alias: String,
    meter_bank: Arc<MeterBank>,
) -> Result<Stream, cpal::Error> {
    let _ = fatal_error;
    let err_fn = move |err| {
        tracing::warn!("output stream error: {err}");
    };

    let shared = Arc::new((Mutex::new(consumers), route_meta));
    let meter_bank_for_cb = meter_bank.clone();
    let alias_for_cb = device_alias;

    let stream = match sample_format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &OutputCallbackInfo| {
                output_callback(data, out_channels, &shared, limiter);
                update_output_meters(data, out_channels, &alias_for_cb, &meter_bank_for_cb);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &OutputCallbackInfo| {
                output_callback(data, out_channels, &shared, limiter);
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &OutputCallbackInfo| {
                output_callback(data, out_channels, &shared, limiter);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I32 => device.build_output_stream(
            config,
            move |data: &mut [i32], _: &OutputCallbackInfo| {
                output_callback(data, out_channels, &shared, limiter);
            },
            err_fn,
            None,
        )?,
        _ => return Err(cpal::Error::new(cpal::ErrorKind::UnsupportedConfig)),
    };

    Ok(stream)
}

fn output_callback<T: FromF32>(
    data: &mut [T],
    out_channels: usize,
    shared: &Arc<(Mutex<Vec<ConsumerEntry>>, Vec<RouteMixMeta>)>,
    limiter: bool,
) {
    if data.is_empty() || out_channels == 0 {
        return;
    }
    let frame_count = data.len() / out_channels;
    let mut mix_buf = vec![0.0f32; data.len()];

    let (consumers, route_meta) = &**shared;

    if let Ok(mut guard) = consumers.lock() {
        for (entry, meta) in guard.iter_mut().zip(route_meta.iter()) {
            let route_channels = entry.channels;
            let mut source_buf = vec![0.0f32; frame_count * route_channels];
            let read = entry.consumer.pop_slice(&mut source_buf);

            // Zero-fill unread portion (underrun → silence).
            for s in &mut source_buf[read..] {
                *s = 0.0;
            }

            mix_route_interleaved(
                &source_buf,
                route_channels,
                &mut mix_buf,
                out_channels,
                &meta.from_channels,
                &meta.to_channels,
                meta.gain,
            );
        }
    }

    if limiter {
        hard_limit_buffer(&mut mix_buf);
    }

    for (dst, src) in data.iter_mut().zip(mix_buf.iter()) {
        *dst = T::from_f32(*src);
    }
}

/// Update per-channel meters from interleaved output data.
fn update_output_meters(data: &[f32], channels: usize, alias: &str, meter_bank: &MeterBank) {
    if channels == 0 {
        return;
    }
    let frames = data.len() / channels;
    for ch in 1..=channels {
        let Some(meter) = meter_bank.get(alias, ch) else {
            continue;
        };
        let ch_samples: Vec<f32> = (0..frames).map(|f| data[f * channels + (ch - 1)]).collect();
        meter.update(&ch_samples);
    }
}

// ─── Sample conversion traits ───────────────────────────────────────────

trait ToF32 {
    fn to_f32(&self) -> f32;
}

impl ToF32 for f32 {
    fn to_f32(&self) -> f32 {
        *self
    }
}

impl ToF32 for i16 {
    fn to_f32(&self) -> f32 {
        *self as f32 / 32768.0
    }
}

impl ToF32 for u16 {
    fn to_f32(&self) -> f32 {
        (*self as f32 - 32768.0) / 32768.0
    }
}

impl ToF32 for i32 {
    fn to_f32(&self) -> f32 {
        *self as f32 / 2147483648.0
    }
}

trait FromF32: Sized {
    fn from_f32(x: f32) -> Self;
}

impl FromF32 for f32 {
    fn from_f32(x: f32) -> Self {
        x
    }
}

impl FromF32 for i16 {
    fn from_f32(x: f32) -> Self {
        (x.clamp(-1.0, 1.0) * 32767.0) as i16
    }
}

impl FromF32 for u16 {
    fn from_f32(x: f32) -> Self {
        ((x.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16
    }
}

impl FromF32 for i32 {
    fn from_f32(x: f32) -> Self {
        (x.clamp(-1.0, 1.0) * 2147483647.0) as i32
    }
}

fn config_watch_dirs(watch_path: &Path, canonical_watch_path: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    push_unique_path(
        &mut dirs,
        watch_path.parent().unwrap_or(Path::new(".")).to_path_buf(),
    );

    if let Some(canonical_watch_path) = canonical_watch_path {
        push_unique_path(
            &mut dirs,
            canonical_watch_path
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf(),
        );
    }

    dirs
}

fn config_event_matches(
    event_paths: &[PathBuf],
    watch_path: &Path,
    canonical_watch_path: Option<&Path>,
) -> bool {
    event_paths
        .iter()
        .any(|p| p == watch_path || canonical_watch_path.is_some_and(|canonical| p == canonical))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

// Use cpal's Sample trait to avoid unused-import warnings.
const _: fn() = || {
    fn _assert_sample<T: Sample>() {}
    fn _check() {
        _assert_sample::<f32>();
    }
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn config_symlink_watches_and_matches_real_target_file() {
        let link_path = PathBuf::from("/tmp/audiorouter-test/link/config.toml");
        let target_path = PathBuf::from("/tmp/audiorouter-test/target/config.toml");

        let dirs = config_watch_dirs(&link_path, Some(&target_path));
        assert!(dirs.contains(&PathBuf::from("/tmp/audiorouter-test/link")));
        assert!(dirs.contains(&PathBuf::from("/tmp/audiorouter-test/target")));

        assert!(config_event_matches(
            std::slice::from_ref(&target_path),
            &link_path,
            Some(&target_path)
        ));
        assert!(config_event_matches(
            std::slice::from_ref(&link_path),
            &link_path,
            Some(&target_path)
        ));
        assert!(!config_event_matches(
            &[PathBuf::from("/tmp/audiorouter-test/target/other.toml")],
            &link_path,
            Some(&target_path)
        ));
    }
}
