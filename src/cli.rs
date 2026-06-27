//! Command-line interface definition and mode selection.

use std::path::PathBuf;

use clap::Parser;

/// `audiorouter` — a macOS-first command-line audio router.
///
/// Reads a TOML configuration file, opens named CoreAudio devices,
/// remaps/mixes audio channels in real time, and writes the mixed result
/// into output devices such as BlackHole.
#[derive(Debug, Parser)]
#[command( version, about, long_about = None)]
pub struct Cli {
    /// TOML configuration file to read.
    ///
    /// If omitted, audiorouter reads the default XDG config path.
    pub config: Option<PathBuf>,

    /// Validate configuration and device availability, then exit.
    #[arg(short = 'n', long)]
    pub check: bool,

    /// List available audio input/output devices, then exit.
    ///
    /// Does not read CONFIG.
    #[arg(short = 'l', long)]
    pub list_devices: bool,

    /// Print the resolved configuration path, then exit.
    #[arg(long)]
    pub print_config_path: bool,

    /// Suppress non-error output.
    #[arg(short, long)]
    pub quiet: bool,

    /// Print extra diagnostics. Repeat for more detail.
    ///
    /// -v:   debug-level tracing (open/close device, route resolution)
    /// -vv:  trace-level (per-callback timing, underrun counts)
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// The four operating modes, selected by the mode flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Check,
    ListDevices,
    PrintConfigPath,
}

impl Cli {
    /// Resolve which mode to run, enforcing mutual-exclusion rules.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - More than one mode flag is set (`--check`, `--list-devices`,
    ///   `--print-config-path`).
    /// - `CONFIG` is supplied alongside `--list-devices`.
    pub fn mode(&self) -> anyhow::Result<Mode> {
        let selected = [self.check, self.list_devices, self.print_config_path]
            .into_iter()
            .filter(|flag| *flag)
            .count();

        if selected > 1 {
            anyhow::bail!(
                "--check, --list-devices, and --print-config-path are mutually exclusive"
            );
        }

        let mode = if self.check {
            Mode::Check
        } else if self.list_devices {
            Mode::ListDevices
        } else if self.print_config_path {
            Mode::PrintConfigPath
        } else {
            Mode::Run
        };

        if self.config.is_some() && matches!(mode, Mode::ListDevices) {
            anyhow::bail!("CONFIG cannot be used with --list-devices");
        }

        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_flags_is_run() {
        let cli = Cli::parse_from(["audiorouter"]);
        assert_eq!(cli.mode().unwrap(), Mode::Run);
    }

    #[test]
    fn check_flag() {
        let cli = Cli::parse_from(["audiorouter", "--check"]);
        assert_eq!(cli.mode().unwrap(), Mode::Check);
    }

    #[test]
    fn check_short_flag() {
        let cli = Cli::parse_from(["audiorouter", "-n"]);
        assert_eq!(cli.mode().unwrap(), Mode::Check);
    }

    #[test]
    fn list_devices_flag() {
        let cli = Cli::parse_from(["audiorouter", "--list-devices"]);
        assert_eq!(cli.mode().unwrap(), Mode::ListDevices);
    }

    #[test]
    fn list_devices_short_flag() {
        let cli = Cli::parse_from(["audiorouter", "-l"]);
        assert_eq!(cli.mode().unwrap(), Mode::ListDevices);
    }

    #[test]
    fn print_config_path_flag() {
        let cli = Cli::parse_from(["audiorouter", "--print-config-path"]);
        assert_eq!(cli.mode().unwrap(), Mode::PrintConfigPath);
    }

    #[test]
    fn check_and_list_devices_fails() {
        let cli = Cli::parse_from(["audiorouter", "--check", "--list-devices"]);
        assert!(cli.mode().is_err());
    }

    #[test]
    fn check_and_print_config_path_fails() {
        let cli = Cli::parse_from(["audiorouter", "--check", "--print-config-path"]);
        assert!(cli.mode().is_err());
    }

    #[test]
    fn list_devices_and_print_config_path_fails() {
        let cli = Cli::parse_from(["audiorouter", "--list-devices", "--print-config-path"]);
        assert!(cli.mode().is_err());
    }

    #[test]
    fn config_with_list_devices_fails() {
        let cli = Cli::parse_from(["audiorouter", "--list-devices", "config.toml"]);
        assert!(cli.mode().is_err());
    }

    #[test]
    fn config_with_check_allowed() {
        let cli = Cli::parse_from(["audiorouter", "--check", "config.toml"]);
        assert_eq!(cli.mode().unwrap(), Mode::Check);
    }

    #[test]
    fn config_with_print_config_path_allowed() {
        let cli = Cli::parse_from(["audiorouter", "--print-config-path", "config.toml"]);
        assert_eq!(cli.mode().unwrap(), Mode::PrintConfigPath);
    }

    #[test]
    fn config_with_run_allowed() {
        let cli = Cli::parse_from(["audiorouter", "config.toml"]);
        assert_eq!(cli.mode().unwrap(), Mode::Run);
    }
}
