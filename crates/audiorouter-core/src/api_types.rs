//! JSON DTOs used by the dashboard HTTP API.
//!
//! These types intentionally preserve the editable TOML shape instead of the
//! runtime-resolved [`crate::config::Config`] shape. In particular, an empty
//! device `name` means the key was omitted in TOML and the runtime alias falls
//! back to `device`.

use std::path::Path;

use serde::{Deserialize, Serialize};

// ── Runtime state ─────────────────────────────────────────────────────────────

/// Live state of the audio engine, pushed to the dashboard via SSE.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub state: RuntimeState,
    pub disabled_route_indices: Vec<usize>,
    pub unavailable_inputs: Vec<String>,
    pub unavailable_outputs: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    #[default]
    Starting,
    Running,
    Stopped,
    FatalError,
}

use std::collections::HashSet;

use crate::config::{
    Config, DEFAULT_BUFFER_SIZE, DEFAULT_SAMPLE_RATE, DeviceConfig, EngineConfig, RouteConfig,
};
use crate::device_inventory::list_audio_devices;
use crate::devices::resolve_devices;
use crate::error::ErrorKind;
use crate::validate::validate_config;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DashboardConfig {
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub devices: Vec<DashboardDeviceConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DashboardDeviceConfig {
    #[serde(default)]
    pub name: String,
    pub device: String,
    #[serde(default)]
    pub limiter: bool,
}

impl From<DashboardConfig> for Config {
    fn from(value: DashboardConfig) -> Self {
        Self {
            engine: value.engine,
            devices: value
                .devices
                .into_iter()
                .map(|device| {
                    let name = if device.name.is_empty() {
                        device.device.clone()
                    } else {
                        device.name
                    };
                    DeviceConfig {
                        name,
                        device: device.device,
                        limiter: device.limiter,
                    }
                })
                .collect(),
            routes: value.routes,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigLoadResponse {
    pub config: DashboardConfig,
    pub raw: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigSaveRequest {
    pub config: DashboardConfig,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSaveResponse {
    pub ok: bool,
    pub raw: String,
    pub errors: Vec<ApiValidationError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigPreviewResponse {
    pub raw: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatusResponse {
    pub errors: Vec<ApiValidationError>,
    pub warnings: Vec<ApiValidationWarning>,
    pub unavailable_inputs: Vec<String>,
    pub unavailable_outputs: Vec<String>,
    pub disabled_route_indices: Vec<usize>,
    pub missing_device_aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidateResponse {
    pub errors: Vec<ApiValidationError>,
    pub warnings: Vec<ApiValidationWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiValidationError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiValidationWarning {
    pub path: String,
    pub message: String,
}

pub fn read_dashboard_config(path: &Path) -> anyhow::Result<(DashboardConfig, String)> {
    let raw = std::fs::read_to_string(path)?;
    let config = toml::from_str(&raw)?;
    Ok((config, raw))
}

pub fn stringify_dashboard_config(config: &DashboardConfig) -> anyhow::Result<String> {
    let mut root = toml::Table::new();

    let mut engine = toml::Table::new();
    if config.engine.sample_rate != DEFAULT_SAMPLE_RATE {
        engine.insert(
            "sample_rate".to_string(),
            toml::Value::Integer(config.engine.sample_rate.into()),
        );
    }
    if config.engine.buffer_size != DEFAULT_BUFFER_SIZE {
        engine.insert(
            "buffer_size".to_string(),
            toml::Value::Integer(config.engine.buffer_size.into()),
        );
    }
    if !engine.is_empty() {
        root.insert("engine".to_string(), toml::Value::Table(engine));
    }

    let route_aliases: HashSet<&str> = config
        .routes
        .iter()
        .flat_map(|route| [route.from.as_str(), route.to.as_str()])
        .collect();

    let devices: Vec<toml::Value> = config
        .devices
        .iter()
        .filter(|device| !is_device_implicit(device, &route_aliases))
        .map(device_to_toml)
        .collect();
    if !devices.is_empty() {
        root.insert("devices".to_string(), toml::Value::Array(devices));
    }

    let routes: Vec<toml::Value> = config.routes.iter().map(route_to_toml).collect();
    if !routes.is_empty() {
        root.insert("routes".to_string(), toml::Value::Array(routes));
    }

    Ok(toml::to_string_pretty(&toml::Value::Table(root))?)
}

fn is_device_implicit(device: &DashboardDeviceConfig, route_aliases: &HashSet<&str>) -> bool {
    (device.name.is_empty() || device.name == device.device)
        && !device.limiter
        && route_aliases.contains(device.device.as_str())
}

fn device_to_toml(device: &DashboardDeviceConfig) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "device".to_string(),
        toml::Value::String(device.device.clone()),
    );
    if !device.name.is_empty() && device.name != device.device {
        table.insert("name".to_string(), toml::Value::String(device.name.clone()));
    }
    if device.limiter {
        table.insert("limiter".to_string(), toml::Value::Boolean(true));
    }
    toml::Value::Table(table)
}

fn route_to_toml(route: &RouteConfig) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert("from".to_string(), toml::Value::String(route.from.clone()));
    table.insert("to".to_string(), toml::Value::String(route.to.clone()));
    table.insert(
        "from_channels".to_string(),
        usize_vec_to_toml_array(&route.from_channels),
    );
    table.insert(
        "to_channels".to_string(),
        usize_vec_to_toml_array(&route.to_channels),
    );
    if route.gain_db != 0.0 {
        table.insert(
            "gain_db".to_string(),
            toml::Value::Float(route.gain_db.into()),
        );
    }
    if route.mute {
        table.insert("mute".to_string(), toml::Value::Boolean(true));
    }
    toml::Value::Table(table)
}

fn usize_vec_to_toml_array(values: &[usize]) -> toml::Value {
    toml::Value::Array(
        values
            .iter()
            .map(|&value| toml::Value::Integer(value as i64))
            .collect(),
    )
}

pub fn validate_dashboard_config(config: DashboardConfig) -> ValidateResponse {
    match validate_config(config.into()) {
        Ok(plan) => ValidateResponse {
            errors: Vec::new(),
            warnings: plan.warnings.into_iter().map(validation_warning).collect(),
        },
        Err(errors) => ValidateResponse {
            errors: errors.into_iter().map(validation_error).collect(),
            warnings: Vec::new(),
        },
    }
}

pub fn dashboard_config_status(config: DashboardConfig) -> ConfigStatusResponse {
    let runtime_config: Config = config.into();
    let plan = match validate_config(runtime_config) {
        Ok(plan) => plan,
        Err(errors) => {
            return ConfigStatusResponse {
                errors: errors.into_iter().map(validation_error).collect(),
                warnings: Vec::new(),
                unavailable_inputs: Vec::new(),
                unavailable_outputs: Vec::new(),
                disabled_route_indices: Vec::new(),
                missing_device_aliases: Vec::new(),
            };
        }
    };

    let inventory = list_audio_devices();
    let mut missing_device_aliases = Vec::new();
    if let Ok(inventory) = &inventory {
        let available_names: HashSet<&str> = inventory
            .all
            .iter()
            .map(|device| device.name.as_str())
            .collect();
        missing_device_aliases = plan
            .devices
            .iter()
            .filter(|device| {
                !device.device.is_empty() && !available_names.contains(device.device.as_str())
            })
            .map(|device| device.name.clone())
            .collect();
        missing_device_aliases.sort();
    }

    match resolve_devices(&plan) {
        Ok(resolved) => {
            let mut warnings: Vec<ApiValidationWarning> =
                plan.warnings.into_iter().map(validation_warning).collect();
            warnings.extend(
                resolved
                    .connect_warnings
                    .into_iter()
                    .map(validation_warning),
            );

            let mut unavailable_inputs: Vec<String> =
                resolved.unavailable_inputs.into_iter().collect();
            let mut unavailable_outputs: Vec<String> =
                resolved.unavailable_outputs.into_iter().collect();
            let mut disabled_route_indices: Vec<usize> =
                resolved.disabled_route_indices.into_iter().collect();
            unavailable_inputs.sort();
            unavailable_outputs.sort();
            disabled_route_indices.sort_unstable();

            ConfigStatusResponse {
                errors: Vec::new(),
                warnings,
                unavailable_inputs,
                unavailable_outputs,
                disabled_route_indices,
                missing_device_aliases,
            }
        }
        Err(error) => {
            let mut warnings: Vec<ApiValidationWarning> =
                plan.warnings.into_iter().map(validation_warning).collect();
            let errors = match error.kind {
                ErrorKind::Config => vec![validation_error(error.message)],
                ErrorKind::Runtime => {
                    warnings.push(validation_warning(error.message));
                    Vec::new()
                }
            };
            ConfigStatusResponse {
                errors,
                warnings,
                unavailable_inputs: Vec::new(),
                unavailable_outputs: Vec::new(),
                disabled_route_indices: Vec::new(),
                missing_device_aliases,
            }
        }
    }
}

fn validation_error(message: String) -> ApiValidationError {
    ApiValidationError {
        path: String::new(),
        message,
    }
}

fn validation_warning(message: String) -> ApiValidationWarning {
    ApiValidationWarning {
        path: String::new(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(from: &str, to: &str) -> RouteConfig {
        RouteConfig {
            from: from.to_string(),
            to: to.to_string(),
            from_channels: vec![1, 2],
            to_channels: vec![1, 2],
            gain_db: 0.0,
            mute: false,
        }
    }

    #[test]
    fn stringify_omits_engine_when_defaults() {
        let config = DashboardConfig {
            engine: EngineConfig::default(),
            devices: Vec::new(),
            routes: Vec::new(),
        };

        let toml = stringify_dashboard_config(&config).unwrap();
        assert!(!toml.contains("[engine]"));
        assert!(!toml.contains("sample_rate"));
        assert!(!toml.contains("buffer_size"));
    }

    #[test]
    fn stringify_keeps_only_non_default_engine_keys() {
        let config = DashboardConfig {
            engine: EngineConfig {
                sample_rate: 96_000,
                buffer_size: DEFAULT_BUFFER_SIZE,
            },
            devices: Vec::new(),
            routes: Vec::new(),
        };

        let toml = stringify_dashboard_config(&config).unwrap();
        assert!(toml.contains("[engine]"));
        assert!(toml.contains("sample_rate = 96000"));
        assert!(!toml.contains("buffer_size"));
    }

    #[test]
    fn stringify_omits_implicit_devices_but_keeps_custom_or_non_default_devices() {
        let config = DashboardConfig {
            engine: EngineConfig::default(),
            devices: vec![
                DashboardDeviceConfig {
                    name: String::new(),
                    device: "VT-4".to_string(),
                    limiter: false,
                },
                DashboardDeviceConfig {
                    name: String::new(),
                    device: "BlackHole 2ch".to_string(),
                    limiter: true,
                },
                DashboardDeviceConfig {
                    name: "mic".to_string(),
                    device: "MacBook Mic".to_string(),
                    limiter: false,
                },
                DashboardDeviceConfig {
                    name: String::new(),
                    device: "Unused".to_string(),
                    limiter: false,
                },
            ],
            routes: vec![
                route("VT-4", "BlackHole 2ch"),
                route("mic", "BlackHole 2ch"),
            ],
        };

        let toml = stringify_dashboard_config(&config).unwrap();
        assert!(!toml.contains("device = \"VT-4\""));
        assert!(toml.contains("device = \"BlackHole 2ch\""));
        assert!(toml.contains("limiter = true"));
        assert!(toml.contains("device = \"MacBook Mic\""));
        assert!(toml.contains("name = \"mic\""));
        assert!(toml.contains("device = \"Unused\""));
    }

    #[test]
    fn stringify_omits_default_route_fields_and_keeps_non_defaults() {
        let mut non_default = route("a", "b");
        non_default.gain_db = -6.0;
        non_default.mute = true;
        let config = DashboardConfig {
            engine: EngineConfig::default(),
            devices: Vec::new(),
            routes: vec![route("x", "y"), non_default],
        };

        let toml = stringify_dashboard_config(&config).unwrap();
        assert_eq!(toml.matches("gain_db").count(), 1);
        assert_eq!(toml.matches("mute").count(), 1);
        assert!(toml.contains("gain_db = -6.0"));
        assert!(toml.contains("mute = true"));
    }

    #[test]
    fn minified_output_round_trips_through_runtime_validation() {
        let config = DashboardConfig {
            engine: EngineConfig::default(),
            devices: vec![DashboardDeviceConfig {
                name: String::new(),
                device: "VT-4".to_string(),
                limiter: false,
            }],
            routes: vec![route("VT-4", "BlackHole 2ch")],
        };

        let toml = stringify_dashboard_config(&config).unwrap();
        let runtime_config: Config = toml::from_str(&toml).unwrap();
        let validated = validate_config(runtime_config).unwrap();
        assert!(validated.device_by_name("VT-4").is_some());
        assert!(validated.device_by_name("BlackHole 2ch").is_some());
    }
}
