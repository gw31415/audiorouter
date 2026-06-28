//! CPAL device enumeration and resolution.
//!
//! This module bridges the validated config plan to actual CoreAudio devices.

use std::collections::{HashMap, HashSet};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SupportedStreamConfig, SupportedStreamConfigRange};

use crate::ui;
use crate::validate::ValidatedConfig;

/// Print all available input and output devices. Does not read config.
///
/// # Errors
///
/// Returns an error if the CPAL host or device enumeration fails.
pub fn print_devices() -> anyhow::Result<()> {
    let host = cpal::default_host();

    ui::header("Input devices");
    let default_input = host.default_input_device();
    let default_input_name = default_input.as_ref().map(|d| d.to_string());
    print_device_list(&host, true, default_input_name.as_deref())?;

    ui::separator();
    ui::header("Output devices");
    let default_output = host.default_output_device();
    let default_output_name = default_output.as_ref().map(|d| d.to_string());
    print_device_list(&host, false, default_output_name.as_deref())?;

    Ok(())
}

fn print_device_list(
    host: &Host,
    is_input: bool,
    default_name: Option<&str>,
) -> anyhow::Result<()> {
    let devices = collect_devices(host, is_input)?;
    for device in &devices {
        print_single_device(device, is_input, default_name)?;
    }
    Ok(())
}

fn print_single_device(
    device: &Device,
    is_input: bool,
    default_name: Option<&str>,
) -> anyhow::Result<()> {
    let name = device.to_string();
    let marker = match default_name {
        Some(dn) if dn == name => Some("default"),
        _ => None,
    };
    let channel_kind = if is_input { "in" } else { "out" };

    let configs = supported_configs(device, is_input);
    match configs {
        Ok(configs) => {
            let max_channels = configs.iter().map(|c| c.channels()).max().unwrap_or(0);
            let rates = collect_sample_rates(device, is_input).unwrap_or_default();
            ui::device_entry(&name, max_channels, channel_kind, Some(&rates), marker);
        }
        Err(e) => {
            ui::device_entry_unavailable(&name, marker, &format!("{e}"));
        }
    }

    Ok(())
}

fn collect_sample_rates(device: &Device, is_input: bool) -> anyhow::Result<Vec<String>> {
    let configs = supported_configs(device, is_input)?;

    let mut rates: Vec<String> = Vec::new();
    for c in configs {
        let min = c.min_sample_rate();
        let max = c.max_sample_rate();
        if min == max {
            rates.push(format!("{} Hz", min));
        } else {
            rates.push(format!("{}–{} Hz", min, max));
        }
    }
    Ok(rates)
}

/// Information about a resolved audio device.
#[derive(Clone)]
#[allow(dead_code)]
pub struct ResolvedDevice {
    /// Config-local alias.
    pub alias: String,
    /// Actual CPAL device name.
    pub name: String,
    /// The CPAL device handle.
    pub device: Device,
    /// Whether this device is used as an input.
    pub is_input: bool,
    /// Whether this device is used as an output.
    pub is_output: bool,
    /// Max input channels available.
    pub max_input_channels: u16,
    /// Max output channels available.
    pub max_output_channels: u16,
}

/// A fully resolved set of devices, ready for stream opening.
pub struct ResolvedAudioDevices {
    pub devices: HashMap<String, ResolvedDevice>,
    /// Warnings about config-defined devices that are not currently connected.
    pub connect_warnings: Vec<String>,
    /// Route indices disabled because at least one endpoint device is not connected.
    pub disabled_route_indices: HashSet<usize>,
    /// Route-referenced input aliases that are currently unavailable.
    pub unavailable_inputs: HashSet<String>,
    /// Route-referenced output aliases that are currently unavailable.
    pub unavailable_outputs: HashSet<String>,
}

impl ResolvedAudioDevices {
    /// All device aliases that are currently missing (not connected).
    /// Combines unavailable inputs and outputs.
    pub fn missing_device_aliases(&self) -> HashSet<String> {
        let mut missing = self.unavailable_inputs.clone();
        missing.extend(self.unavailable_outputs.iter().cloned());
        missing
    }

    /// Returns true when the route at `index` is active for stream construction.
    pub fn route_enabled(&self, index: usize) -> bool {
        !self.disabled_route_indices.contains(&index)
    }

    /// Number of active routes after connectivity pruning.
    pub fn active_route_count(&self, plan: &ValidatedConfig) -> usize {
        plan.routes
            .iter()
            .enumerate()
            .filter(|(i, _)| self.route_enabled(*i))
            .count()
    }

    /// All resolved device aliases that need an input stream.
    pub fn input_device_names(&self) -> Vec<&str> {
        self.devices
            .values()
            .filter(|d| d.is_input)
            .map(|d| d.alias.as_str())
            .collect()
    }

    /// All resolved device aliases that need an output stream.
    pub fn output_device_names(&self) -> Vec<&str> {
        self.devices
            .values()
            .filter(|d| d.is_output)
            .map(|d| d.alias.as_str())
            .collect()
    }

    /// Human-readable device connectivity changes between two resolutions.
    pub fn connectivity_events(
        &self,
        next: &ResolvedAudioDevices,
        plan: &ValidatedConfig,
    ) -> Vec<String> {
        let mut events = Vec::new();

        for alias in self.unavailable_inputs.difference(&next.unavailable_inputs) {
            events.push(format_device_event(plan, alias, "input", "connected"));
        }
        for alias in next.unavailable_inputs.difference(&self.unavailable_inputs) {
            events.push(format_device_event(plan, alias, "input", "disconnected"));
        }
        for alias in self
            .unavailable_outputs
            .difference(&next.unavailable_outputs)
        {
            events.push(format_device_event(plan, alias, "output", "connected"));
        }
        for alias in next
            .unavailable_outputs
            .difference(&self.unavailable_outputs)
        {
            events.push(format_device_event(plan, alias, "output", "disconnected"));
        }

        events.sort();
        events
    }
}

fn format_device_event(plan: &ValidatedConfig, alias: &str, side: &str, state: &str) -> String {
    let device = plan
        .device_by_name(alias)
        .map(|role| role.device.as_str())
        .unwrap_or(alias);
    format!("device \"{alias}\" (\"{device}\") {state} as {side}")
}

/// Resolve all devices in the validated config against actual CPAL devices.
///
/// For each device that `needs_input`, search input devices by exact name.
/// For each device that `needs_output`, search output devices by exact name.
/// Then validate channel counts and sample rate.
///
/// Devices **not used by any route** (neither input nor output) are checked
/// for connectivity only — if they are not currently found among system
/// devices, a warning is added to [`ResolvedAudioDevices::connect_warnings`]
/// instead of returning an error. This lets users keep a config with optional
/// devices that may be plugged in later.
///
/// # Errors
///
/// Missing route-referenced devices are warnings, not errors: every route that
/// uses the missing input/output side is disabled. Still returns a `Config`
/// error when a connected device has insufficient channels or an unsupported
/// sample rate. Returns a `Runtime` error for CPAL enumeration failures.
pub fn resolve_devices(
    plan: &ValidatedConfig,
) -> Result<ResolvedAudioDevices, crate::error::AppError> {
    let host = cpal::default_host();

    let input_devices = collect_devices(&host, true).map_err(|e| {
        crate::error::AppError::runtime(format!("failed to enumerate input devices: {e}"))
    })?;
    let output_devices = collect_devices(&host, false).map_err(|e| {
        crate::error::AppError::runtime(format!("failed to enumerate output devices: {e}"))
    })?;

    let sample_rate = plan.config.engine.sample_rate;

    let mut resolved: HashMap<String, ResolvedDevice> = HashMap::new();
    let mut connect_warnings: Vec<String> = Vec::new();
    let mut unavailable_inputs: HashSet<String> = HashSet::new();
    let mut unavailable_outputs: HashSet<String> = HashSet::new();

    // First pass: identify route endpoint sides that are currently missing.
    // Missing input disables routes that read from that alias; missing output
    // disables routes that write to that alias. If the same device is still
    // available in the opposite direction, routes using that side may continue.
    for role in &plan.devices {
        let dev_name = &role.device;

        if role.needs_input && !input_devices.iter().any(|d| &d.to_string() == dev_name) {
            unavailable_inputs.insert(role.name.clone());
            connect_warnings.push(format!(
                "device \"{}\" (\"{}\") is not currently connected as input; related routes disabled",
                role.name, dev_name
            ));
        }

        if role.needs_output && !output_devices.iter().any(|d| &d.to_string() == dev_name) {
            unavailable_outputs.insert(role.name.clone());
            connect_warnings.push(format!(
                "device \"{}\" (\"{}\") is not currently connected as output; related routes disabled",
                role.name, dev_name
            ));
        }
    }

    let disabled_route_indices: HashSet<usize> = plan
        .routes
        .iter()
        .enumerate()
        .filter_map(|(i, route)| {
            if unavailable_inputs.contains(&route.from) || unavailable_outputs.contains(&route.to) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    if !disabled_route_indices.is_empty() {
        connect_warnings.push(format!(
            "{} route(s) disabled because required audio devices are not connected",
            disabled_route_indices.len()
        ));
    }

    for role in &plan.devices {
        let dev_name = &role.device;

        let mut cpal_input_device: Option<Device> = None;
        let mut max_in_ch: u16 = 0;
        let mut cpal_output_device: Option<Device> = None;
        let mut max_out_ch: u16 = 0;

        let active_input_routes: Vec<_> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(i, r)| !disabled_route_indices.contains(i) && r.from == role.name)
            .map(|(_, r)| r)
            .collect();
        let active_output_routes: Vec<_> = plan
            .routes
            .iter()
            .enumerate()
            .filter(|(i, r)| !disabled_route_indices.contains(i) && r.to == role.name)
            .map(|(_, r)| r)
            .collect();
        let needs_input = !active_input_routes.is_empty();
        let needs_output = !active_output_routes.is_empty();
        let required_input_channels = active_input_routes
            .iter()
            .flat_map(|r| r.from_channels.iter())
            .copied()
            .max()
            .unwrap_or(0);
        let required_output_channels = active_output_routes
            .iter()
            .flat_map(|r| r.to_channels.iter())
            .copied()
            .max()
            .unwrap_or(0);

        if needs_input {
            let found = input_devices.iter().find(|d| &d.to_string() == dev_name);
            match found {
                Some(d) => {
                    let max_ch = max_channels(d, true).unwrap_or(0);
                    if max_ch < required_input_channels as u16 {
                        return Err(crate::error::AppError::config(format!(
                            "device alias \"{}\" uses CoreAudio device \"{}\" as input requiring {} channel(s), \
                             but only {} input channel(s) are available",
                            role.name, dev_name, required_input_channels, max_ch
                        )));
                    }
                    if !supports_sample_rate(d, true, sample_rate) {
                        return Err(crate::error::AppError::config(format!(
                            "device \"{}\" does not support the configured sample rate {} Hz",
                            dev_name, sample_rate
                        )));
                    }
                    max_in_ch = max_ch;
                    cpal_input_device = Some(d.clone());
                }
                None => continue,
            }
        }

        if needs_output {
            let found = output_devices.iter().find(|d| &d.to_string() == dev_name);
            match found {
                Some(d) => {
                    let max_ch = max_channels(d, false).unwrap_or(0);
                    if max_ch < required_output_channels as u16 {
                        return Err(crate::error::AppError::config(format!(
                            "output device \"{}\" resolved to \"{}\", \
                             but route requires output channel {}",
                            role.name, dev_name, required_output_channels
                        )));
                    }
                    if !supports_sample_rate(d, false, sample_rate) {
                        return Err(crate::error::AppError::config(format!(
                            "device \"{}\" does not support the configured sample rate {} Hz",
                            dev_name, sample_rate
                        )));
                    }
                    max_out_ch = max_ch;
                    cpal_output_device = Some(d.clone());
                }
                None => continue,
            }
        }

        // Devices not used by any route: check connectivity, warn if absent.
        if !needs_input && !needs_output {
            if unavailable_inputs.contains(&role.name) || unavailable_outputs.contains(&role.name) {
                continue;
            }
            let found_as_input = input_devices.iter().any(|d| &d.to_string() == dev_name);
            let found_as_output = output_devices.iter().any(|d| &d.to_string() == dev_name);
            if !found_as_input && !found_as_output {
                connect_warnings.push(format!(
                    "device \"{}\" (\"{}\") is not currently connected",
                    role.name, dev_name
                ));
            }
            continue;
        }

        // Probe physical channel counts for the unused direction (display only, never fails).
        if max_in_ch == 0 {
            if let Some(d) = input_devices.iter().find(|d| &d.to_string() == dev_name) {
                max_in_ch = max_channels(d, true).unwrap_or(0);
            }
        }
        if max_out_ch == 0 {
            if let Some(d) = output_devices.iter().find(|d| &d.to_string() == dev_name) {
                max_out_ch = max_channels(d, false).unwrap_or(0);
            }
        }

        let device = cpal_input_device
            .or(cpal_output_device)
            .expect("at least one role must be active");

        resolved.insert(
            role.name.clone(),
            ResolvedDevice {
                alias: role.name.clone(),
                name: role.device.clone(),
                device,
                is_input: needs_input,
                is_output: needs_output,
                max_input_channels: max_in_ch,
                max_output_channels: max_out_ch,
            },
        );
    }

    Ok(ResolvedAudioDevices {
        devices: resolved,
        connect_warnings,
        disabled_route_indices,
        unavailable_inputs,
        unavailable_outputs,
    })
}

/// Find the best supported stream config for a device at the given sample rate.
#[allow(dead_code)]
pub fn find_stream_config(
    device: &Device,
    is_input: bool,
    sample_rate: u32,
    _desired_buffer_size: u32,
) -> anyhow::Result<SupportedStreamConfig> {
    let supported_configs = supported_configs(device, is_input)?;

    for config_range in supported_configs {
        let min = config_range.min_sample_rate();
        let max = config_range.max_sample_rate();
        if sample_rate >= min && sample_rate <= max {
            return Ok(config_range.with_sample_rate(sample_rate));
        }
    }

    anyhow::bail!(
        "no supported config found for device \"{}\" at {} Hz",
        device,
        sample_rate
    )
}

fn collect_devices(host: &Host, is_input: bool) -> anyhow::Result<Vec<Device>> {
    let mut result = Vec::new();
    if is_input {
        for device in host.input_devices()? {
            result.push(device);
        }
    } else {
        for device in host.output_devices()? {
            result.push(device);
        }
    }
    Ok(result)
}

fn max_channels(device: &Device, is_input: bool) -> Option<u16> {
    let configs = supported_configs(device, is_input).ok()?;
    configs.iter().map(|c| c.channels()).max()
}

fn supports_sample_rate(device: &Device, is_input: bool, rate: u32) -> bool {
    let Ok(configs) = supported_configs(device, is_input) else {
        return true;
    };
    for c in configs {
        if rate >= c.min_sample_rate() && rate <= c.max_sample_rate() {
            return true;
        }
    }
    false
}

/// Collect supported stream config ranges into a Vec, handling the
/// input/output type mismatch by collecting eagerly.
fn supported_configs(
    device: &Device,
    is_input: bool,
) -> anyhow::Result<Vec<SupportedStreamConfigRange>> {
    if is_input {
        Ok(device.supported_input_configs()?.collect())
    } else {
        Ok(device.supported_output_configs()?.collect())
    }
}

/// Play silence to a device briefly to verify it can be opened.
///
/// Used by `--check` mode to confirm stream viability without keeping a
/// long-running stream alive.
#[allow(dead_code)]
pub fn verify_device_openable(
    device: &Device,
    is_input: bool,
    sample_rate: u32,
) -> anyhow::Result<()> {
    let config = find_stream_config(device, is_input, sample_rate, 256)?;
    let stream_config = cpal::StreamConfig {
        channels: config.channels(),
        sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let err_fn = |err| ui::error(format!("stream error: {err}"));

    if is_input {
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
                stream_config,
                |_d: &[f32], _i: &cpal::InputCallbackInfo| {},
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                stream_config,
                |_d: &[i16], _i: &cpal::InputCallbackInfo| {},
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
                stream_config,
                |_d: &[u16], _i: &cpal::InputCallbackInfo| {},
                err_fn,
                None,
            )?,
            _ => anyhow::bail!("unsupported sample format"),
        };
        stream.play()?;
        drop(stream);
    } else {
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream::<f32, _, _>(
                stream_config,
                |_d: &mut [f32], _i: &cpal::OutputCallbackInfo| {},
                err_fn,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_output_stream::<i16, _, _>(
                stream_config,
                |_d: &mut [i16], _i: &cpal::OutputCallbackInfo| {},
                err_fn,
                None,
            )?,
            cpal::SampleFormat::U16 => device.build_output_stream::<u16, _, _>(
                stream_config,
                |_d: &mut [u16], _i: &cpal::OutputCallbackInfo| {},
                err_fn,
                None,
            )?,
            _ => anyhow::bail!("unsupported sample format"),
        };
        stream.play()?;
        drop(stream);
    }

    Ok(())
}
