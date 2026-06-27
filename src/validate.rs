//! Pure config validation and device role inference.
//!
//! This module does not touch CPAL. All validation here is about internal
//! config consistency — device aliases, channel numbers, route references,
//! and role/channel-count inference.

use std::collections::HashMap;

use crate::config::{Config, DeviceConfig, RouteConfig};

/// A device with its inferred input/output roles and required channel counts.
#[derive(Debug, Clone)]
pub struct ResolvedDeviceRole {
    pub name: String,
    pub device: String,
    pub limiter: bool,
    pub needs_input: bool,
    pub needs_output: bool,
    pub required_input_channels: usize,
    pub required_output_channels: usize,
}

/// A route that has passed pure validation.
#[derive(Debug, Clone)]
pub struct ValidatedRoute {
    pub from: String,
    pub to: String,
    pub from_channels: Vec<usize>,
    pub to_channels: Vec<usize>,
    pub gain_db: f32,
    pub mute: bool,
}

/// The output of successful validation.
#[derive(Debug, Clone)]
pub struct ValidatedConfig {
    pub config: Config,
    pub devices: Vec<ResolvedDeviceRole>,
    pub routes: Vec<ValidatedRoute>,
    pub warnings: Vec<String>,
}

impl ValidatedConfig {
    /// Returns the resolved device role for the given alias.
    pub fn device_by_name(&self, name: &str) -> Option<&ResolvedDeviceRole> {
        self.devices.iter().find(|d| d.name == name)
    }

    /// All device aliases that need an input stream.
    pub fn input_device_names(&self) -> Vec<&str> {
        self.devices
            .iter()
            .filter(|d| d.needs_input)
            .map(|d| d.name.as_str())
            .collect()
    }

    /// All device aliases that need an output stream.
    pub fn output_device_names(&self) -> Vec<&str> {
        self.devices
            .iter()
            .filter(|d| d.needs_output)
            .map(|d| d.name.as_str())
            .collect()
    }
}

/// Run pure config validation, returning a [`ValidatedConfig`] or a list of
/// error messages.
///
/// This function does not touch CPAL. It validates internal consistency only.
pub fn validate_config(config: Config) -> Result<ValidatedConfig, Vec<String>> {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // --- engine ---
    if config.engine.sample_rate == 0 {
        errors.push("engine.sample_rate must be positive".to_string());
    }
    if config.engine.buffer_size == 0 {
        errors.push("engine.buffer_size must be positive".to_string());
    }

    // --- devices ---
    if config.devices.is_empty() {
        errors.push("config must define at least one device".to_string());
    }

    let mut name_map: HashMap<&str, &DeviceConfig> = HashMap::new();
    for dev in &config.devices {
        if dev.name.is_empty() {
            errors.push("device name must be non-empty".to_string());
        }
        if dev.device.is_empty() {
            errors.push(format!(
                "device \"{}\" has an empty 'device' field (CoreAudio device name)",
                dev.name
            ));
        }
        if let Some(_existing) = name_map.get(dev.name.as_str()) {
            errors.push(format!(
                "duplicate device name \"{}\"; names must be unique",
                dev.name
            ));
        } else {
            name_map.insert(dev.name.as_str(), dev);
        }
    }

    // --- routes ---
    if config.routes.is_empty() {
        errors.push("config must define at least one route".to_string());
    }

    for (i, route) in config.routes.iter().enumerate() {
        validate_route(i, route, &name_map, &mut errors);
    }

    // Return early if there are structural errors.
    if !errors.is_empty() {
        return Err(errors);
    }

    // --- role inference ---
    let mut roles: HashMap<String, ResolvedDeviceRole> = config
        .devices
        .iter()
        .map(|d| {
            (
                d.name.clone(),
                ResolvedDeviceRole {
                    name: d.name.clone(),
                    device: d.device.clone(),
                    limiter: d.limiter,
                    needs_input: false,
                    needs_output: false,
                    required_input_channels: 0,
                    required_output_channels: 0,
                },
            )
        })
        .collect();

    for route in &config.routes {
        if let Some(role) = roles.get_mut(&route.from) {
            role.needs_input = true;
            for &ch in &route.from_channels {
                if ch > role.required_input_channels {
                    role.required_input_channels = ch;
                }
            }
        }
        if let Some(role) = roles.get_mut(&route.to) {
            role.needs_output = true;
            for &ch in &route.to_channels {
                if ch > role.required_output_channels {
                    role.required_output_channels = ch;
                }
            }
        }
    }

    // --- warnings ---
    for role in roles.values() {
        if !role.needs_input && !role.needs_output {
            warnings.push(format!("device \"{}\" is not used by any route", role.name));
        }
    }

    if !config.engine.sample_rate_in_recommended_range() {
        warnings.push(format!(
            "engine.sample_rate {} is outside the recommended range (44100 or 48000)",
            config.engine.sample_rate
        ));
    }
    if !config.engine.buffer_size_in_recommended_range() {
        warnings.push(format!(
            "engine.buffer_size {} is outside the recommended range (64..=2048)",
            config.engine.buffer_size
        ));
    }

    let devices: Vec<ResolvedDeviceRole> = config
        .devices
        .iter()
        .map(|d| roles.get(&d.name).cloned().unwrap())
        .collect();

    let routes: Vec<ValidatedRoute> = config
        .routes
        .iter()
        .map(|r| ValidatedRoute {
            from: r.from.clone(),
            to: r.to.clone(),
            from_channels: r.from_channels.clone(),
            to_channels: r.to_channels.clone(),
            gain_db: r.gain_db,
            mute: r.mute,
        })
        .collect();

    Ok(ValidatedConfig {
        config,
        devices,
        routes,
        warnings,
    })
}

fn validate_route(
    i: usize,
    route: &RouteConfig,
    name_map: &HashMap<&str, &DeviceConfig>,
    errors: &mut Vec<String>,
) {
    let known: Vec<&str> = name_map.keys().copied().collect();

    if !name_map.contains_key(route.from.as_str()) {
        errors.push(format!(
            "route[{i}].from references unknown device alias \"{}\"; known devices: {}",
            route.from,
            known.join(", ")
        ));
    }
    if !name_map.contains_key(route.to.as_str()) {
        errors.push(format!(
            "route[{i}].to references unknown device alias \"{}\"; known devices: {}",
            route.to,
            known.join(", ")
        ));
    }

    if route.from == route.to {
        errors.push(format!(
            "route[{i}].from and route[{i}].to are both \"{}\"; same-device routes are rejected in v0.1 to prevent feedback",
            route.from
        ));
    }

    if route.from_channels.is_empty() {
        errors.push(format!("route[{i}].from_channels is empty",));
    }
    if route.to_channels.is_empty() {
        errors.push(format!("route[{i}].to_channels is empty",));
    }

    if route.from_channels.len() != route.to_channels.len() {
        errors.push(format!(
            "route[{i}] maps from_channels length {} to to_channels length {}; lengths must match. \
             Use from_channels = [1, 1] for mono-to-stereo.",
            route.from_channels.len(),
            route.to_channels.len()
        ));
    }

    for &ch in &route.from_channels {
        if ch == 0 {
            errors.push(format!(
                "route[{i}].from_channels contains invalid channel 0; channels are 1-based"
            ));
        }
    }
    for &ch in &route.to_channels {
        if ch == 0 {
            errors.push(format!(
                "route[{i}].to_channels contains invalid channel 0; channels are 1-based"
            ));
        }
    }

    if !route.gain_db.is_finite() {
        errors.push(format!(
            "route[{i}].gain_db is not a finite number (NaN or infinity rejected)"
        ));
    }
}

// ─── helpers on EngineConfig for recommended-range warnings ───────────────

trait EngineConfigExt {
    fn sample_rate_in_recommended_range(&self) -> bool;
    fn buffer_size_in_recommended_range(&self) -> bool;
}

impl EngineConfigExt for crate::config::EngineConfig {
    fn sample_rate_in_recommended_range(&self) -> bool {
        matches!(self.sample_rate, 44100 | 48000)
    }

    fn buffer_size_in_recommended_range(&self) -> bool {
        (64..=2048).contains(&self.buffer_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> Config {
        toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "src"
device = "Source"

[[devices]]
name = "dst"
device = "Dest"

[[routes]]
from = "src"
to = "dst"
from_channels = [1, 2]
to_channels = [1, 2]
"#,
        )
        .unwrap()
    }

    #[test]
    fn valid_config_passes() {
        let config = make_config();
        let result = validate_config(config).unwrap();
        assert_eq!(result.devices.len(), 2);
        assert_eq!(result.routes.len(), 1);
    }

    #[test]
    fn duplicate_device_names_fail() {
        let mut config = make_config();
        config.devices[1].name = "src".to_string();
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("duplicate")));
    }

    #[test]
    fn unknown_route_from_fails() {
        let mut config = make_config();
        config.routes[0].from = "nonexistent".to_string();
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("unknown device alias")));
    }

    #[test]
    fn unknown_route_to_fails() {
        let mut config = make_config();
        config.routes[0].to = "nonexistent".to_string();
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("unknown device alias")));
    }

    #[test]
    fn mismatched_channel_lengths_fail() {
        let mut config = make_config();
        config.routes[0].to_channels = vec![1, 2, 3];
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("lengths must match")));
    }

    #[test]
    fn zero_channel_fails() {
        let mut config = make_config();
        config.routes[0].from_channels = vec![0, 2];
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("invalid channel 0")));
    }

    #[test]
    fn same_device_route_fails() {
        let mut config = make_config();
        config.routes[0].to = "src".to_string();
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("same-device routes")));
    }

    #[test]
    fn nan_gain_fails() {
        let mut config = make_config();
        config.routes[0].gain_db = f32::NAN;
        let result = validate_config(config);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors.iter().any(|e| e.contains("not a finite number")));
    }

    #[test]
    fn inf_gain_fails() {
        let mut config = make_config();
        config.routes[0].gain_db = f32::INFINITY;
        let result = validate_config(config);
        assert!(result.is_err());
    }

    #[test]
    fn role_inference_from_only() {
        let config = make_config();
        let result = validate_config(config).unwrap();
        let src = result.device_by_name("src").unwrap();
        let dst = result.device_by_name("dst").unwrap();
        assert!(src.needs_input);
        assert!(!src.needs_output);
        assert!(!dst.needs_input);
        assert!(dst.needs_output);
    }

    #[test]
    fn role_inference_both() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "a"
device = "DevA"

[[devices]]
name = "b"
device = "DevB"

[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]

[[routes]]
from = "b"
to = "a"
from_channels = [1]
to_channels = [1]
"#,
        )
        .unwrap();
        let result = validate_config(config).unwrap();
        let a = result.device_by_name("a").unwrap();
        let b = result.device_by_name("b").unwrap();
        assert!(a.needs_input && a.needs_output);
        assert!(b.needs_input && b.needs_output);
    }

    #[test]
    fn required_channel_counts() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "src"
device = "Source"

[[devices]]
name = "dst"
device = "Dest"

[[routes]]
from = "src"
to = "dst"
from_channels = [3, 4]
to_channels = [1, 2]
"#,
        )
        .unwrap();
        let result = validate_config(config).unwrap();
        let src = result.device_by_name("src").unwrap();
        let dst = result.device_by_name("dst").unwrap();
        assert_eq!(src.required_input_channels, 4);
        assert_eq!(dst.required_output_channels, 2);
    }

    #[test]
    fn unused_device_warning() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "src"
device = "Source"

[[devices]]
name = "dst"
device = "Dest"

[[devices]]
name = "unused"
device = "Unused"

[[routes]]
from = "src"
to = "dst"
from_channels = [1]
to_channels = [1]
"#,
        )
        .unwrap();
        let result = validate_config(config).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.contains("not used by any route"))
        );
    }

    #[test]
    fn empty_devices_fails() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[routes]]
from = "a"
to = "b"
from_channels = [1]
to_channels = [1]
"#,
        )
        .unwrap();
        assert!(validate_config(config).is_err());
    }

    #[test]
    fn empty_routes_fails() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "a"
device = "DevA"
"#,
        )
        .unwrap();
        assert!(validate_config(config).is_err());
    }

    #[test]
    fn zero_sample_rate_fails() {
        let mut config = make_config();
        config.engine.sample_rate = 0;
        assert!(validate_config(config).is_err());
    }

    #[test]
    fn zero_buffer_size_fails() {
        let mut config = make_config();
        config.engine.buffer_size = 0;
        assert!(validate_config(config).is_err());
    }
}
