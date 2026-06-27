//! Audio engine: stream startup/shutdown, ring buffers, real-time mixing.
//!
//! Uses one SPSC ring buffer per route (SPEC.md §8.3, option 1) to support
//! fan-out (one input feeding multiple outputs). The input callback for device
//! `D` writes its full physical interleaved frames into every route buffer
//! where `D` is `from`. The output callback for device `O` reads from every
//! route buffer where `O` is `to`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    Device, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat, SampleRate, Stream,
    StreamConfig,
};
use ringbuf::{HeapCons, HeapProd, HeapRb, traits::*};

use crate::mixer::{db_to_linear, hard_limit_buffer, mix_route_interleaved};
use crate::ui;
use crate::validate::ValidatedConfig;

/// Run the audio engine until SIGINT, a fatal error, or a config change.
///
/// When the config file changes on disk, the process self-restarts via `exec`
/// so that the new config takes effect without the user needing to manually
/// stop and restart.
///
/// # Errors
///
/// Returns `AppError` (Runtime) on fatal audio/stream errors.
pub fn run_audio(
    plan: &ValidatedConfig,
    resolved: &crate::devices::ResolvedAudioDevices,
    config_path: &std::path::Path,
) -> Result<(), crate::error::AppError> {
    let running = Arc::new(AtomicBool::new(true));
    let fatal_error = Arc::new(AtomicBool::new(false));

    {
        let r = running.clone();
        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .map_err(|e| {
            crate::error::AppError::runtime(format!("failed to install Ctrl-C handler: {e}"))
        })?;
    }

    let sample_rate = plan.config.engine.sample_rate;
    let buffer_size = plan.config.engine.buffer_size;

    // ─── Pre-split all per-route ring buffers ───────────────────────────
    //
    // Each route gets one SPSC ring. We split immediately into (prod, cons)
    // and store them in separate maps keyed by route index.

    let mut route_producers: HashMap<usize, HeapProd<f32>> = HashMap::new();
    let mut route_consumers: HashMap<usize, (HeapCons<f32>, usize)> = HashMap::new();
    let mut route_input_channels: HashMap<usize, usize> = HashMap::new();

    for (i, route) in plan.routes.iter().enumerate() {
        let from_device = &resolved.devices[&route.from];
        let channels = if from_device.is_input && from_device.max_input_channels > 0 {
            from_device.max_input_channels as usize
        } else {
            plan.device_by_name(&route.from)
                .map(|r| r.required_input_channels.max(1))
                .unwrap_or(1)
        };

        let capacity = buffer_size as usize * channels * 4;
        let rb = HeapRb::<f32>::new(capacity);
        let (prod, cons) = rb.split();

        route_producers.insert(i, prod);
        route_consumers.insert(i, (cons, channels));
        route_input_channels.insert(i, channels);
    }

    let mut input_streams: Option<Vec<Stream>> = None;
    let mut output_streams: Option<Vec<Stream>> = None;

    // ─── Open input streams (one per input device) ──────────────────────

    for alias in plan.input_device_names() {
        let resolved_dev = &resolved.devices[alias];

        let route_indices: Vec<usize> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(_, r)| r.from == alias)
            .map(|(i, _)| i)
            .collect();

        let channels = route_input_channels[&route_indices[0]];

        let supported = find_config_for(&resolved_dev.device, true, sample_rate, channels)
            .map_err(crate::error::AppError::runtime)?;

        let stream_config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let sample_format = supported.sample_format();

        // Collect producers for this device's routes.
        let mut producers: Vec<HeapProd<f32>> = Vec::new();
        for &ri in &route_indices {
            let prod = route_producers.remove(&ri).unwrap();
            producers.push(prod);
        }

        let stream = build_input_stream(
            &resolved_dev.device,
            &stream_config,
            sample_format,
            producers,
            &fatal_error,
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

        input_streams.get_or_insert_with(Vec::new).push(stream);
    }

    // ─── Open output streams (one per output device) ────────────────────

    for alias in plan.output_device_names() {
        let resolved_dev = &resolved.devices[alias];

        let route_indices: Vec<usize> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(_, r)| r.to == alias)
            .map(|(i, _)| i)
            .collect();

        let out_channels = resolved_dev.max_output_channels as usize;
        let supported = find_config_for(&resolved_dev.device, false, sample_rate, out_channels)
            .map_err(crate::error::AppError::runtime)?;

        let stream_config = StreamConfig {
            channels: supported.channels(),
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let sample_format = supported.sample_format();

        // Collect consumers and metadata for this device's routes.
        let mut consumers: Vec<ConsumerEntry> = Vec::new();
        let mut route_meta: Vec<RouteMixMeta> = Vec::new();
        for &ri in &route_indices {
            let (cons, ch) = route_consumers.remove(&ri).unwrap();
            consumers.push(ConsumerEntry {
                consumer: cons,
                channels: ch,
            });

            let route = &plan.routes[ri];
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

        let limiter = plan
            .device_by_name(alias)
            .map(|d| d.limiter)
            .unwrap_or(false);

        let stream = build_output_stream(
            &resolved_dev.device,
            &stream_config,
            sample_format,
            out_channels,
            consumers,
            route_meta,
            limiter,
            &fatal_error,
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

        output_streams.get_or_insert_with(Vec::new).push(stream);
    }

    // ─── Config file watcher ────────────────────────────────────────────
    //
    // Watch the config file for changes. When a write/close-write event fires,
    // set the `config_changed` flag. The main loop checks this flag and, after
    // a short debounce, self-restarts via `exec` so the new config takes effect.

    let config_changed = Arc::new(AtomicBool::new(false));

    {
        let config_changed = config_changed.clone();
        let watch_path = config_path.to_path_buf();
        // notify::Watcher must not be dropped until the loop ends, so spawn a
        // dedicated thread that owns the watcher and blocks on its event channel.
        std::thread::spawn(move || {
            use notify::{EventKind, RecursiveMode, Watcher};

            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    ui::warning(format!("config watch disabled: {e}"));
                    return;
                }
            };

            let canonical_watch_path = std::fs::canonicalize(&watch_path).ok();

            // Watch parent directories so renames/atomic saves work. For symlinked
            // config files, also watch the real target's parent; otherwise writes to
            // the target can happen outside the symlink's directory and never emit an
            // event on the symlink path itself.
            for watch_dir in config_watch_dirs(&watch_path, canonical_watch_path.as_deref()) {
                if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                    ui::warning(format!("config watch disabled: {e}"));
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
                        config_changed.store(true, Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        });
    }

    // ─── Main loop ──────────────────────────────────────────────────────

    let mut last_change: Option<std::time::Instant> = None;
    let debounce = Duration::from_millis(500);

    loop {
        if !running.load(Ordering::SeqCst) || fatal_error.load(Ordering::SeqCst) {
            break;
        }

        // Record a new filesystem change event.
        if config_changed.load(Ordering::SeqCst) {
            last_change = Some(std::time::Instant::now());
            config_changed.store(false, Ordering::SeqCst);
        }

        // Check whether the debounce window has elapsed since the last change.
        // This runs every iteration regardless of config_changed, so we don't
        // miss the restart window after clearing the flag.
        if let Some(prev) = last_change
            && std::time::Instant::now().duration_since(prev) >= debounce
        {
            // Config file has settled — restart.
            ui::success("config changed — restarting");
            drop(input_streams.take());
            drop(output_streams.take());
            self_restart(config_path);
            // self_restart only returns on failure — exit the loop.
            break;
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    drop(input_streams.take());
    drop(output_streams.take());

    if fatal_error.load(Ordering::SeqCst) {
        return Err(crate::error::AppError::runtime(
            "fatal audio stream error occurred",
        ));
    }

    Ok(())
}

/// Restart the current process via `execvp` so the new config takes effect.
///
/// Drops the current process image and replaces it in-place. If exec fails,
/// prints a warning and returns (caller falls through to normal shutdown).
fn self_restart(config_path: &std::path::Path) {
    use std::os::unix::process::CommandExt;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            ui::error(format!("restart failed: cannot find current exe: {e}"));
            return;
        }
    };

    let config_str = config_path.to_string_lossy().into_owned();

    let err = std::process::Command::new(&exe).arg(&config_str).exec();

    // exec only returns on failure.
    ui::error(format!("restart failed: {err}"));
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
        let min = range.min_sample_rate().0;
        let max = range.max_sample_rate().0;
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
            let min = range.min_sample_rate().0;
            let max = range.max_sample_rate().0;
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

    Ok(range.with_sample_rate(SampleRate(sample_rate)))
}

// ─── Input stream ────────────────────────────────────────────────────────

fn build_input_stream(
    device: &Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    producers: Vec<HeapProd<f32>>,
    fatal_error: &Arc<AtomicBool>,
) -> Result<Stream, cpal::BuildStreamError> {
    let fatal = fatal_error.clone();
    let err_fn = move |err| {
        ui::error(format!("input stream: {err}"));
        fatal.store(true, Ordering::SeqCst);
    };

    let producers = Arc::new(Mutex::new(producers));

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &InputCallbackInfo| {
                input_callback(data, &producers);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &InputCallbackInfo| {
                input_callback(data, &producers);
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &InputCallbackInfo| {
                input_callback(data, &producers);
            },
            err_fn,
            None,
        )?,
        SampleFormat::I32 => device.build_input_stream(
            config,
            move |data: &[i32], _: &InputCallbackInfo| {
                input_callback(data, &producers);
            },
            err_fn,
            None,
        )?,
        _ => return Err(cpal::BuildStreamError::StreamConfigNotSupported),
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

// ─── Output stream ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn build_output_stream(
    device: &Device,
    config: &StreamConfig,
    sample_format: SampleFormat,
    out_channels: usize,
    consumers: Vec<ConsumerEntry>,
    route_meta: Vec<RouteMixMeta>,
    limiter: bool,
    fatal_error: &Arc<AtomicBool>,
) -> Result<Stream, cpal::BuildStreamError> {
    let fatal = fatal_error.clone();
    let err_fn = move |err| {
        ui::error(format!("output stream: {err}"));
        fatal.store(true, Ordering::SeqCst);
    };

    let shared = Arc::new((Mutex::new(consumers), route_meta));

    let stream = match sample_format {
        SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &OutputCallbackInfo| {
                output_callback(data, out_channels, &shared, limiter);
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
        _ => return Err(cpal::BuildStreamError::StreamConfigNotSupported),
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
        // Consumers and route_meta are in the same order (both filtered by
        // this output device's routes in the same iteration).
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
