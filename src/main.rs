//! `audiorouter` — entrypoint: parse args, dispatch mode, map errors to exit codes.

mod audio;
mod cli;
mod config;
mod devices;
mod error;
mod mixer;
mod ui;
mod validate;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::{Cli, Mode};
use crate::config::{read_config, resolve_config_path};
use crate::error::{AppError, exit_code_for};
use crate::validate::validate_config;

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Initialize logging level.
    init_logging(&cli);

    match cli.mode() {
        Ok(mode) => match dispatch(&cli, mode) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                ui::error(&e.message);
                ExitCode::from(exit_code_for(e.kind) as u8)
            }
        },
        Err(e) => {
            ui::error(format!("{e}"));
            ExitCode::from(1u8)
        }
    }
}

fn init_logging(cli: &Cli) {
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

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn dispatch(cli: &Cli, mode: Mode) -> Result<(), AppError> {
    match mode {
        Mode::PrintConfigPath => run_print_config_path(cli),
        Mode::ListDevices => run_list_devices(),
        Mode::Check => run_check(cli),
        Mode::Run => run_run(cli),
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
        plan.devices.len(),
        plan.routes.len(),
        plan.config.engine.sample_rate,
        plan.config.engine.buffer_size,
    ));

    ui::separator();

    let inputs: Vec<&str> = plan.input_device_names();
    let outputs: Vec<&str> = plan.output_device_names();

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
                format!("→ {} ({}ch out{})", dev.device, dev.required_output_channels, limiter_tag),
            );
        }
    }

    // resolved is used for validation; its existence is the proof.
    let _ = resolved;

    Ok(())
}

fn run_run(cli: &Cli) -> Result<(), AppError> {
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

    let resolved = devices::resolve_devices(&plan)?;

    // Print device connectivity warnings.
    for w in &resolved.connect_warnings {
        ui::warning(w);
    }

    // Startup summary (unless --quiet).
    if !cli.quiet {
        print_startup_summary(&path, &plan);
    }

    audio::run_audio(&plan, &resolved, &path)?;

    if !cli.quiet {
        ui::success("stopped");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_startup_summary(path: &std::path::Path, plan: &validate::ValidatedConfig) {
    ui::header("audiorouter");
    ui::separator();

    ui::kv("config", format!("{}", path.display()));
    ui::kv(
        "engine",
        format!("{} Hz · buffer {}", plan.config.engine.sample_rate, plan.config.engine.buffer_size),
    );
    ui::separator();

    let inputs = plan.input_device_names();
    if !inputs.is_empty() {
        ui::header("Inputs");
        for &alias in &inputs {
            let dev = plan.device_by_name(alias).unwrap();
            ui::item_with_detail(
                alias,
                format!("→ {} ({}ch)", dev.device, dev.required_input_channels),
            );
        }
    }

    let outputs = plan.output_device_names();
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
                format!("→ {} ({}ch{})", dev.device, dev.required_output_channels, limiter_tag),
            );
        }
    }

    ui::separator();
    ui::header("Routes");
    for r in &plan.routes {
        ui::route(&r.from, &r.from_channels, &r.to, &r.to_channels, r.gain_db, r.mute);
    }

    ui::separator();
    ui::success("running — Ctrl-C to stop");
}
