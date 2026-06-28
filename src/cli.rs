//! Command-line interface definition and subcommand selection.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// `audiorouter` — a cross-platform command-line audio router.
///
/// Reads a TOML configuration file, opens named audio devices,
/// remaps/mixes audio channels in real time, and writes the mixed result
/// into virtual or physical output devices.
///
/// When no subcommand is given, `run` is assumed.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// TOML configuration file to read.
    ///
    /// If omitted, audiorouter reads the default XDG config path.
    #[arg(short, long, global = true)]
    pub config: Option<PathBuf>,

    /// Suppress non-error output.
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Print extra diagnostics. Repeat for more detail.
    ///
    /// -v:   debug-level tracing (open/close device, route resolution)
    /// -vv:  trace-level (per-callback timing, underrun counts)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Operating modes, selected by subcommand.
///
/// When no subcommand is given, [`Command::Run`] is assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// Start audio routing (default when no subcommand is given).
    Run,

    /// Validate configuration and device availability, then exit.
    Check,

    /// List available audio input/output devices, then exit.
    ///
    /// Does not require a config file.
    ListDevices,

    /// Print the resolved configuration path, then exit.
    ConfigPath,
}

impl Cli {
    /// Return the resolved command, defaulting to [`Command::Run`] when
    /// no subcommand was specified.
    pub fn command_or_default(&self) -> Command {
        self.command.unwrap_or(Command::Run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_subcommand_is_run() {
        let cli = Cli::parse_from(["audiorouter"]);
        assert_eq!(cli.command_or_default(), Command::Run);
    }

    #[test]
    fn run_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "run"]);
        assert_eq!(cli.command_or_default(), Command::Run);
    }

    #[test]
    fn check_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "check"]);
        assert_eq!(cli.command_or_default(), Command::Check);
    }

    #[test]
    fn list_devices_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "list-devices"]);
        assert_eq!(cli.command_or_default(), Command::ListDevices);
    }

    #[test]
    fn config_path_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "config-path"]);
        assert_eq!(cli.command_or_default(), Command::ConfigPath);
    }

    // --- global flags work before and after the subcommand ---

    #[test]
    fn config_flag_before_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "-c", "cfg.toml", "check"]);
        assert_eq!(cli.command_or_default(), Command::Check);
        assert_eq!(cli.config, Some(PathBuf::from("cfg.toml")));
    }

    #[test]
    fn config_flag_after_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "check", "-c", "cfg.toml"]);
        assert_eq!(cli.command_or_default(), Command::Check);
        assert_eq!(cli.config, Some(PathBuf::from("cfg.toml")));
    }

    #[test]
    fn config_flag_without_subcommand() {
        let cli = Cli::parse_from(["audiorouter", "-c", "cfg.toml"]);
        assert_eq!(cli.command_or_default(), Command::Run);
        assert_eq!(cli.config, Some(PathBuf::from("cfg.toml")));
    }

    #[test]
    fn quiet_flag() {
        let cli = Cli::parse_from(["audiorouter", "--quiet", "run"]);
        assert!(cli.quiet);
    }

    #[test]
    fn verbose_flags() {
        let cli = Cli::parse_from(["audiorouter", "-vv", "run"]);
        assert_eq!(cli.verbose, 2);
    }
}
