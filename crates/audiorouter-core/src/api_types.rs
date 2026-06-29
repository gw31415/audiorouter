//! JSON DTOs used by the dashboard HTTP API.
//!
//! These types intentionally preserve the editable TOML shape instead of the
//! runtime-resolved [`crate::config::Config`] shape. In particular, an empty
//! device `name` means the key was omitted in TOML and the runtime alias falls
//! back to `device`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{Config, DeviceConfig, EngineConfig, RouteConfig};
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
    Ok(toml::to_string_pretty(config)?)
}

pub fn validate_dashboard_config(config: DashboardConfig) -> ValidateResponse {
    match validate_config(config.into()) {
        Ok(plan) => ValidateResponse {
            errors: Vec::new(),
            warnings: plan
                .warnings
                .into_iter()
                .map(|message| ApiValidationWarning {
                    path: String::new(),
                    message,
                })
                .collect(),
        },
        Err(errors) => ValidateResponse {
            errors: errors
                .into_iter()
                .map(|message| ApiValidationError {
                    path: String::new(),
                    message,
                })
                .collect(),
            warnings: Vec::new(),
        },
    }
}
