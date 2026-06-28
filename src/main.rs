//! `audiorouter` — entrypoint: parse args, dispatch mode, map errors to exit codes.

mod audio;
mod cli;
mod config;
mod devices;
mod error;
mod graph;
mod log_buffer;
mod meter;
mod mixer;
mod tui;
mod ui;
mod validate;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use clap::{CommandFactory, FromArgMatches};

use crate::cli::{Cli, Command};
use crate::config::{read_config, resolve_config_path};
use crate::error::{AppError, exit_code_for};
use crate::validate::validate_config;

const APP_VERSION: &str = match option_env!("APP_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> ExitCode {
    let matches = Cli::command().version(APP_VERSION).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let command = cli.command_or_default();
    let interactive = is_interactive_run(command);

    // Initialize logging level and destination.
    init_logging(&cli, command, interactive);

    match dispatch(&cli, command, interactive) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            ui::error(&e.message);
            ExitCode::from(exit_code_for(e.kind) as u8)
        }
    }
}

fn init_logging(cli: &Cli, command: Command, interactive: bool) {
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

fn is_interactive_run(command: Command) -> bool {
    matches!(command, Command::Run)
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
}

fn dispatch(cli: &Cli, command: Command, interactive: bool) -> Result<(), AppError> {
    match command {
        Command::ConfigPath => run_print_config_path(cli),
        Command::ListDevices => run_list_devices(),
        Command::Check => run_check(cli),
        Command::Run => run_run(cli, interactive),
    }
}

fn run_print_config_path(cli: &Cli) -> Result<(), AppError> {
    let path = resolve_config_path(cli.config.as_deref())
        .map_err(|e| AppError::runtime(format!("cannot resolve config path: {e}")))?;
    println!("{}", path.display());
    Ok(())
}

fn run_list_devices() -> Result<(), AppError> {
    devices::print_devices().map_err(|e| AppError::runtime(format!("{e}")))
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

fn run_run(cli: &Cli, interactive: bool) -> Result<(), AppError> {
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

    // Build the audio engine.
    let engine = audio::AudioEngine::new(plan, resolved, &path)?;

    if interactive {
        // Run the TUI event loop (handles its own Ctrl-C / quit).
        tui::run(engine, &path, &warnings)?;
    } else {
        run_headless(engine)?;
    }

    Ok(())
}

fn run_headless(mut engine: audio::AudioEngine) -> Result<(), AppError> {
    tracing::info!("audiorouter started in non-interactive mode");

    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let running_for_handler = running.clone();
    ctrlc::set_handler(move || {
        running_for_handler.store(false, std::sync::atomic::Ordering::SeqCst);
    })
    .map_err(|e| AppError::runtime(format!("failed to install Ctrl-C handler: {e}")))?;

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));

        for event in engine.refresh_devices()? {
            tracing::info!("{event}");
        }

        match engine.state() {
            audio::EngineState::Running => {}
            audio::EngineState::Stopped => break,
            audio::EngineState::FatalError => {
                return Err(AppError::runtime("fatal audio stream error"));
            }
        }
    }

    engine.stop();
    tracing::info!("audiorouter stopped");
    Ok(())
}
