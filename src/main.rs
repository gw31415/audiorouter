//! `audiorouter` — entrypoint: parse args, dispatch mode, map errors to exit codes.

mod audio;
mod cli;
mod engine_actor;
pub use audiorouter_core::{RuntimeSnapshot, RuntimeState, config, devices, error, validate};
mod graph;
mod log_buffer;
mod meter;
mod mixer;
mod tui;
mod ui;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};

use crate::cli::{Cli, Command};
use crate::config::{read_config, resolve_config_path};
use crate::engine_actor::{EngineCmd, EngineHandle};
use crate::error::{AppError, exit_code_for};
use crate::validate::validate_config;

const APP_VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[tokio::main]
async fn main() -> ExitCode {
    let matches = Cli::command().version(APP_VERSION).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let command = cli.command_or_default();
    let interactive = is_interactive_run(&command);

    // Initialize logging level and destination.
    init_logging(&cli, &command, interactive);

    match dispatch(&cli, command, interactive).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ui::error(&e.message);
            ExitCode::from(exit_code_for(e.kind) as u8)
        }
    }
}

fn init_logging(cli: &Cli, command: &Command, interactive: bool) {
    use tracing_subscriber::EnvFilter;

    let default_level = if cli.quiet {
        "error"
    } else {
        match cli.verbose {
            0 => "warn",
            1 => "debug",
            _ => "trace",
        }
    };

    let filter = EnvFilter::try_new(default_level).unwrap_or_else(|_| EnvFilter::new("warn"));

    if matches!(command, Command::Run) && interactive {
        log_buffer::init();
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .with_ansi(false)
            .compact()
            .with_writer(log_buffer::TuiLogMakeWriter)
            .init();
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn is_interactive_run(command: &Command) -> bool {
    matches!(command, Command::Run)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

async fn dispatch(cli: &Cli, command: Command, interactive: bool) -> Result<(), AppError> {
    match command {
        Command::ConfigPath => run_print_config_path(cli),
        Command::ListDevices => run_list_devices(cli),
        Command::Check => run_check(cli),
        Command::Run => run_run(cli, interactive).await,
        Command::Completions { shell, output } => run_completions(shell, output.as_deref()),
    }
}

fn run_completions(
    shell: Option<clap_complete::Shell>,
    output: Option<&std::path::Path>,
) -> Result<(), AppError> {
    use clap::CommandFactory;
    use clap_complete::generate;
    use std::io::Write;

    let shell = shell.or_else(detect_shell).ok_or_else(|| {
        AppError::runtime("could not detect current shell; pass one explicitly: bash, fish, zsh, …")
    })?;

    let mut cmd = Cli::command().version(APP_VERSION);

    if output.is_none_or(|p| p == std::path::Path::new("-")) {
        // stdout: wrap to silently swallow BrokenPipe so `| head` etc. don't panic.
        struct BrokenPipeSink<W: Write>(W);
        impl<W: Write> Write for BrokenPipeSink<W> {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                match self.0.write(buf) {
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(buf.len()),
                    r => r,
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                match self.0.flush() {
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                    r => r,
                }
            }
        }
        generate(
            shell,
            &mut cmd,
            "audiorouter",
            &mut BrokenPipeSink(std::io::stdout()),
        );
    } else {
        let path = output.unwrap();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::runtime(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let mut file = std::fs::File::create(path)
            .map_err(|e| AppError::runtime(format!("cannot create {}: {e}", path.display())))?;
        generate(shell, &mut cmd, "audiorouter", &mut file);
        file.flush().ok();
        ui::success(path.display().to_string());
    }
    Ok(())
}

fn detect_shell() -> Option<clap_complete::Shell> {
    use clap_complete::Shell;
    let path = std::env::var("SHELL").ok()?;
    match std::path::Path::new(&path).file_name()?.to_str()? {
        "bash" => Some(Shell::Bash),
        "fish" => Some(Shell::Fish),
        "zsh" => Some(Shell::Zsh),
        "elvish" => Some(Shell::Elvish),
        "powershell" | "pwsh" => Some(Shell::PowerShell),
        _ => None,
    }
}

fn run_print_config_path(cli: &Cli) -> Result<(), AppError> {
    let path = resolve_config_path(cli.config.as_deref())
        .map_err(|e| AppError::runtime(format!("cannot resolve config path: {e}")))?;
    println!("{}", path.display());
    Ok(())
}

fn run_list_devices(cli: &Cli) -> Result<(), AppError> {
    let inventory =
        audiorouter_core::list_audio_devices().map_err(|e| AppError::runtime(format!("{e}")))?;

    ui::header("Audio devices");
    for device in &inventory.all {
        let marker = match (device.is_default_input, device.is_default_output) {
            (true, true) => Some("default"),
            (true, false) => Some("default in"),
            (false, true) => Some("default out"),
            (false, false) => None,
        };
        if device.max_input_channels == 0 && device.max_output_channels == 0 {
            ui::device_entry_unavailable(&device.name, marker, "no supported configs");
        } else {
            let rates: Vec<String> = Vec::new();
            ui::device_entry(
                &device.name,
                (device.max_input_channels > 0).then_some(device.max_input_channels),
                (device.max_output_channels > 0).then_some(device.max_output_channels),
                (cli.verbose > 0).then_some(rates.as_slice()),
                marker,
            );
        }
    }
    Ok(())
}

fn run_check(cli: &Cli) -> Result<(), AppError> {
    let path = resolve_config_path(cli.config.as_deref())
        .map_err(|e| AppError::runtime(format!("cannot resolve config path: {e}")))?;

    let config = read_config(&path).map_err(|e| AppError::config(format!("{e}")))?;
    let plan = validate_config(config).map_err(|errors| {
        AppError::config(format!("config validation failed:\n{}", errors.join("\n")))
    })?;

    // Print config warnings.
    for w in &plan.warnings {
        ui::warning(w);
    }

    // Resolve devices via CPAL.
    let resolved = devices::resolve_devices(&plan)?;

    // Print device connectivity warnings.
    for w in &resolved.connect_warnings {
        ui::warning(w);
    }

    // Print success summary.
    ui::success(format!(
        "config ok — {} devices, {} routes, {} Hz, buffer {}",
        resolved.devices.len(),
        resolved.active_route_count(&plan),
        plan.config.engine.sample_rate,
        plan.config.engine.buffer_size,
    ));

    ui::separator();

    let inputs: Vec<&str> = resolved.input_device_names();
    let outputs: Vec<&str> = resolved.output_device_names();

    if !inputs.is_empty() {
        ui::header("Inputs");
        for &alias in &inputs {
            let dev = plan.device_by_name(alias).unwrap();
            ui::item_with_detail(
                alias,
                format!("→ {} ({}ch in)", dev.device, dev.required_input_channels),
            );
        }
    }

    if !outputs.is_empty() {
        if !inputs.is_empty() {
            ui::separator();
        }
        ui::header("Outputs");
        for &alias in &outputs {
            let dev = plan.device_by_name(alias).unwrap();
            let limiter_tag = if dev.limiter { " · limiter" } else { "" };
            ui::item_with_detail(
                alias,
                format!(
                    "→ {} ({}ch out{})",
                    dev.device, dev.required_output_channels, limiter_tag
                ),
            );
        }
    }

    // resolved is used for validation; its existence is the proof.
    let _ = resolved;

    Ok(())
}

async fn run_run(cli: &Cli, interactive: bool) -> Result<(), AppError> {
    let path = resolve_config_path(cli.config.as_deref())
        .map_err(|e| AppError::runtime(format!("cannot resolve config path: {e}")))?;

    let config = read_config(&path).map_err(|e| AppError::config(format!("{e}")))?;
    let plan = validate_config(config).map_err(|errors| {
        AppError::config(format!("config validation failed:\n{}", errors.join("\n")))
    })?;

    if !interactive {
        for w in &plan.warnings {
            ui::warning(w);
        }
    }

    let resolved = devices::resolve_devices(&plan)?;

    if !interactive {
        for w in &resolved.connect_warnings {
            ui::warning(w);
        }
    }

    // Collect warnings to pass to TUI.
    let mut warnings = plan.warnings.clone();
    warnings.extend(resolved.connect_warnings.iter().cloned());

    // Build the audio engine and spawn the engine actor thread.
    let engine = audio::AudioEngine::new(plan, resolved, &path)?;
    let (handle, engine_thread) = engine_actor::spawn_engine_actor(engine);

    let result = if interactive {
        let path_clone = path.clone();
        let warnings_clone = warnings.clone();
        tokio::task::spawn_blocking(move || tui::run(handle, &path_clone, &warnings_clone))
            .await
            .map_err(|e| AppError::runtime(format!("TUI thread panicked: {e}")))?
    } else {
        run_headless(handle).await
    };

    engine_thread.join().ok();
    result
}

async fn run_headless(handle: EngineHandle) -> Result<(), AppError> {
    tracing::info!("audiorouter started in non-interactive mode");

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                handle.cmd_tx.try_send(EngineCmd::Stop).ok();
                break;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                let state = {
                    let view = handle.shared.read().unwrap();
                    view.snapshot.state.clone()
                };
                match state {
                    RuntimeState::Running | RuntimeState::Starting => {}
                    RuntimeState::Stopped => break,
                    RuntimeState::FatalError => {
                        return Err(AppError::runtime("fatal audio stream error"));
                    }
                }
            }
        }
    }

    tracing::info!("audiorouter stopped");
    Ok(())
}
