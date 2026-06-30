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
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
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

    /// Launch the web dashboard (HTTP/SSE UI) in the default browser.
    ///
    /// By default the dashboard binds to localhost (127.0.0.1) on port 7822.
    Dashboard {
        /// Expose the dashboard on the local network (bind 0.0.0.0).
        ///
        /// Off by default, which keeps the dashboard reachable only from this
        /// machine. Pass `--host` to share it with other devices on the LAN.
        #[arg(long)]
        host: bool,

        /// Port to bind the dashboard server on.
        #[arg(long, short, default_value_t = 7822)]
        port: u16,

        /// Do not open the dashboard in the default browser.
        #[arg(long)]
        no_open: bool,
    },

    /// Generate a shell completion script.
    ///
    /// Writes to stdout by default; use --output to write to a file instead.
    /// When no shell is given the current shell is detected from $SHELL.
    Completions {
        /// Shell to generate completions for [default: current $SHELL].
        shell: Option<clap_complete::Shell>,

        /// Output file [default: stdout].
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
}

impl Cli {
    /// Return the resolved command, defaulting to [`Command::Run`] when
    /// no subcommand was specified.
    pub fn command_or_default(&self) -> Command {
        self.command.clone().unwrap_or(Command::Run)
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

    #[test]
    fn dashboard_subcommand_defaults() {
        let cli = Cli::parse_from(["audiorouter", "dashboard"]);
        assert_eq!(
            cli.command_or_default(),
            Command::Dashboard {
                host: false,
                port: 7822,
                no_open: false,
            }
        );
    }

    #[test]
    fn dashboard_no_open_flag() {
        let cli = Cli::parse_from(["audiorouter", "dashboard", "--no-open"]);
        assert_eq!(
            cli.command_or_default(),
            Command::Dashboard {
                host: false,
                port: 7822,
                no_open: true,
            }
        );
    }

    #[test]
    fn dashboard_host_and_port_flags() {
        let cli = Cli::parse_from([
            "audiorouter",
            "dashboard",
            "--host",
            "--port",
            "9000",
            "--no-open",
        ]);
        assert_eq!(
            cli.command_or_default(),
            Command::Dashboard {
                host: true,
                port: 9000,
                no_open: true,
            }
        );
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
