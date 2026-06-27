//! `audiorouter` — entrypoint: parse args, dispatch mode, map errors to exit codes.

mod audio;
mod cli;
mod config;
mod devices;
mod error;
mod mixer;
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
                eprintln!("error: {e}");
                ExitCode::from(exit_code_for(e.kind) as u8)
            }
        },
        Err(e) => {
            eprintln!("error: {e}");
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

    // Print warnings.
    for w in &plan.warnings {
        eprintln!("warning: {w}");
    }

    // Resolve devices via CPAL.
    let resolved = devices::resolve_devices(&plan)?;

    // Print success summary.
    let inputs: Vec<&str> = plan.input_device_names();
    let outputs: Vec<&str> = plan.output_device_names();

    println!(
        "Config OK: {} devices, {} routes, sample_rate={}, buffer_size={}",
        plan.devices.len(),
        plan.routes.len(),
        plan.config.engine.sample_rate,
        plan.config.engine.buffer_size
    );
    if !inputs.is_empty() {
        let summary: Vec<String> = inputs
            .iter()
            .map(|&alias| {
                let dev = plan.device_by_name(alias).unwrap();
                format!("{} -> {}", alias, dev.device)
            })
            .collect();
        println!("Inputs: {}", summary.join(", "));
    }
    if !outputs.is_empty() {
        let summary: Vec<String> = outputs
            .iter()
            .map(|&alias| {
                let dev = plan.device_by_name(alias).unwrap();
                format!(
                    "{} -> {}{}",
                    alias,
                    dev.device,
                    if dev.limiter { " (limiter)" } else { "" }
                )
            })
            .collect();
        println!("Outputs: {}", summary.join(", "));
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

    // Print warnings.
    for w in &plan.warnings {
        eprintln!("warning: {w}");
    }

    let resolved = devices::resolve_devices(&plan)?;

    // Startup summary (unless --quiet).
    if !cli.quiet {
        print_startup_summary(&path, &plan);
    }

    audio::run_audio(&plan, &resolved)?;

    if !cli.quiet {
        println!("audiorouter: stopped");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_startup_summary(path: &std::path::Path, plan: &validate::ValidatedConfig) {
    println!("Using config: {}", path.display());
    println!(
        "Engine: {} Hz, buffer_size={}",
        plan.config.engine.sample_rate, plan.config.engine.buffer_size
    );

    let inputs = plan.input_device_names();
    if !inputs.is_empty() {
        println!("Inputs:");
        for alias in inputs {
            let dev = plan.device_by_name(alias).unwrap();
            println!(
                "  {} -> {}, required channels: {}",
                alias, dev.device, dev.required_input_channels
            );
        }
    }

    let outputs = plan.output_device_names();
    if !outputs.is_empty() {
        println!("Outputs:");
        for alias in outputs {
            let dev = plan.device_by_name(alias).unwrap();
            println!(
                "  {} -> {}, required channels: {}{}",
                alias,
                dev.device,
                dev.required_output_channels,
                if dev.limiter { ", limiter: true" } else { "" }
            );
        }
    }

    println!("Routes:");
    for r in &plan.routes {
        let fc: Vec<String> = r.from_channels.iter().map(|c| c.to_string()).collect();
        let tc: Vec<String> = r.to_channels.iter().map(|c| c.to_string()).collect();
        let gain_display = if r.mute {
            "muted".to_string()
        } else {
            format!("{:.1} dB", r.gain_db)
        };
        println!(
            "  {} [{}] -> {} [{}], gain={}",
            r.from,
            fc.join(","),
            r.to,
            tc.join(","),
            gain_display
        );
    }
}
