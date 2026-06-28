//! Config structs, path resolution, and TOML parsing.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_SAMPLE_RATE: u32 = 48000;
pub const DEFAULT_BUFFER_SIZE: u32 = 256;

/// Top-level config structure.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

/// `[engine]` — sample rate and buffer size.
#[derive(Debug, Clone, Deserialize)]
pub struct EngineConfig {
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_buffer_size")]
    pub buffer_size: u32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }
}

/// `[[devices]]` — a named device alias.
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    pub name: String,
    pub device: String,
    pub limiter: bool,
}

impl<'de> Deserialize<'de> for DeviceConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawDeviceConfig {
            name: Option<String>,
            device: String,
            #[serde(default)]
            limiter: bool,
        }

        let raw = RawDeviceConfig::deserialize(deserializer)?;
        Ok(Self {
            name: raw.name.unwrap_or_else(|| raw.device.clone()),
            device: raw.device,
            limiter: raw.limiter,
        })
    }
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_buffer_size() -> u32 {
    DEFAULT_BUFFER_SIZE
}

/// `[[routes]]` — a channel mapping from one device to another.
///
/// Channel numbers are stored as `usize` but represent **1-based physical
/// channel numbers**.
#[derive(Debug, Clone, Deserialize)]
pub struct RouteConfig {
    pub from: String,
    pub to: String,
    pub from_channels: Vec<usize>,
    pub to_channels: Vec<usize>,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub mute: bool,
}

/// Resolve the default config path.
///
/// Resolution order:
/// 1. `$XDG_CONFIG_HOME/audiorouter/config.toml` — if `XDG_CONFIG_HOME` is set and non-empty
/// 2. `~/.config/audiorouter/config.toml` — if the file already exists there (XDG fallback,
///    honoured on all platforms so dotfile-managed configs work on macOS/Windows too)
/// 3. Platform-native config directory:
///    - Linux/BSD: `~/.config/audiorouter/config.toml`
///    - macOS:     `~/Library/Application Support/audiorouter/config.toml`
///    - Windows:   `%APPDATA%\audiorouter\config.toml`
///
/// # Errors
///
/// Returns an error if the home/config directory cannot be determined.
pub fn default_config_path() -> anyhow::Result<PathBuf> {
    // 1. Explicit XDG override.
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("audiorouter").join("config.toml"));
    }

    // 2. XDG fallback: ~/.config — honoured on all platforms so users who manage
    //    dotfiles with stow/chezmoi/etc. on macOS or Windows don't need to move files.
    if let Some(home) = dirs::home_dir() {
        let xdg_fallback = home.join(".config").join("audiorouter").join("config.toml");
        if xdg_fallback.exists() {
            return Ok(xdg_fallback);
        }
    }

    // 3. Platform-native directory.
    let config_dir =
        dirs::config_dir().ok_or_else(|| anyhow::anyhow!("cannot determine config directory"))?;
    Ok(config_dir.join("audiorouter").join("config.toml"))
}

/// Resolve a config path from the optional positional `CONFIG` argument or
/// the default path.
///
/// Does **not** call `canonicalize()` so that `--print-config-path` works even
/// when the file does not exist.
///
/// # Errors
///
/// Returns an error if the default path cannot be determined.
pub fn resolve_config_path(config_arg: Option<&Path>) -> anyhow::Result<PathBuf> {
    match config_arg {
        Some(p) => {
            if p.is_absolute() {
                Ok(p.to_path_buf())
            } else {
                let cwd = std::env::current_dir()?;
                Ok(cwd.join(p))
            }
        }
        None => default_config_path(),
    }
}

/// Read and parse a TOML config file.
///
/// # Errors
///
/// Returns an error whose message includes the path (and a hint if the file
/// does not exist) when reading or parsing fails.
pub fn read_config(path: &Path) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read config file {}: {e}\n\
             Hint: Run 'audiorouter --print-config-path' to see the expected location.",
            path.display()
        )
    })?;

    toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse config {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONFIG: &str = r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
name = "vt4"
device = "VT-4"

[[devices]]
name = "mic"
device = "MacBook Pro Microphone"

[[devices]]
name = "blackhole"
device = "BlackHole 2ch"
limiter = true

[[devices]]
name = "speaker"
device = "MacBook Pro Speakers"

[[routes]]
from = "vt4"
to = "blackhole"
from_channels = [3, 4]
to_channels = [1, 2]
gain_db = 0.0

[[routes]]
from = "mic"
to = "blackhole"
from_channels = [1, 1]
to_channels = [1, 2]
gain_db = -8.0

[[routes]]
from = "vt4"
to = "speaker"
from_channels = [3, 4]
to_channels = [1, 2]
gain_db = -12.0
"#;

    #[test]
    fn parse_sample_config() {
        let config: Config = toml::from_str(SAMPLE_CONFIG).unwrap();
        assert_eq!(config.engine.sample_rate, 48000);
        assert_eq!(config.engine.buffer_size, 256);
        assert_eq!(config.devices.len(), 4);
        assert_eq!(config.routes.len(), 3);

        assert_eq!(config.devices[0].name, "vt4");
        assert_eq!(config.devices[0].device, "VT-4");
        assert!(!config.devices[0].limiter);

        assert_eq!(config.devices[2].name, "blackhole");
        assert!(config.devices[2].limiter);

        assert_eq!(config.routes[0].from, "vt4");
        assert_eq!(config.routes[0].to, "blackhole");
        assert_eq!(config.routes[0].from_channels, vec![3, 4]);
        assert_eq!(config.routes[0].to_channels, vec![1, 2]);

        // mono-to-stereo route
        assert_eq!(config.routes[1].from_channels, vec![1, 1]);
        assert_eq!(config.routes[1].to_channels, vec![1, 2]);
        assert!((config.routes[1].gain_db - (-8.0)).abs() < 1e-6);
    }

    #[test]
    fn default_mute_is_false() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 44100
buffer_size = 128

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
"#,
        )
        .unwrap();
        assert!(!config.routes[0].mute);
        assert!((config.routes[0].gain_db - 0.0).abs() < 1e-6);
    }

    #[test]
    fn default_engine_is_used_when_engine_table_is_missing() {
        let config: Config = toml::from_str(
            r#"
[[devices]]
device = "Source"

[[devices]]
device = "Dest"

[[routes]]
from = "Source"
to = "Dest"
from_channels = [1]
to_channels = [1]
"#,
        )
        .unwrap();

        assert_eq!(config.engine.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.engine.buffer_size, DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn default_engine_fields_are_used_when_missing() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 44100

[[devices]]
device = "Source"

[[devices]]
device = "Dest"

[[routes]]
from = "Source"
to = "Dest"
from_channels = [1]
to_channels = [1]
"#,
        )
        .unwrap();

        assert_eq!(config.engine.sample_rate, 44100);
        assert_eq!(config.engine.buffer_size, DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn device_name_defaults_to_device_string() {
        let config: Config = toml::from_str(
            r#"
[engine]
sample_rate = 48000
buffer_size = 256

[[devices]]
device = "BlackHole 2ch"
limiter = true
"#,
        )
        .unwrap();

        assert_eq!(config.devices[0].name, "BlackHole 2ch");
        assert_eq!(config.devices[0].device, "BlackHole 2ch");
        assert!(config.devices[0].limiter);
    }

    #[test]
    fn xdg_config_path() {
        // SAFETY: setenv is safe in single-threaded tests.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg_test");
        }
        let path = default_config_path().unwrap();
        assert_eq!(path, PathBuf::from("/tmp/xdg_test/audiorouter/config.toml"));
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn empty_xdg_falls_back_to_home() {
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", "");
        }
        let path = default_config_path().unwrap();
        assert!(path.ends_with(".config/audiorouter/config.toml"));
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn absolute_config_path_returned_unchanged() {
        let p = Path::new("/absolute/path/config.toml");
        let resolved = resolve_config_path(Some(p)).unwrap();
        assert_eq!(resolved, PathBuf::from("/absolute/path/config.toml"));
    }

    #[test]
    fn relative_config_path_joined_with_cwd() {
        let p = Path::new("relative.toml");
        let resolved = resolve_config_path(Some(p)).unwrap();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolved, cwd.join("relative.toml"));
    }

    #[test]
    fn read_missing_file_includes_hint() {
        let result = read_config(Path::new("/nonexistent/audiorouter-test.toml"));
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("--print-config-path"));
    }
}
