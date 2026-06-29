//! Core audiorouter domain types and services shared by the CLI/TUI and dashboard API.

pub mod api_types;
pub mod config;
pub mod device_inventory;
pub mod devices;
pub mod error;
pub mod validate;

pub use config::{
    Config, DEFAULT_BUFFER_SIZE, DEFAULT_SAMPLE_RATE, DeviceConfig, EngineConfig, RouteConfig,
    default_config_path, read_config, resolve_config_path,
};
pub use device_inventory::{AudioDeviceInfo, DevicesResponse, list_audio_devices};
pub use error::{AppError, ErrorKind, exit_code_for};
pub use validate::{ResolvedDeviceRole, ValidatedConfig, ValidatedRoute, validate_config};
