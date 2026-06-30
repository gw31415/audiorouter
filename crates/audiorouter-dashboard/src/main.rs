use std::net::SocketAddr;
use std::path::PathBuf;

use audiorouter_dashboard::{DashboardState, dashboard_router};

const DEFAULT_ADDR: &str = "127.0.0.1:7822";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = Options::parse()?;
    let state = DashboardState::new(options.config_path);
    let _device_watcher = state.spawn_device_watcher();
    let _config_watcher = state.spawn_config_watcher();
    let listener = tokio::net::TcpListener::bind(options.addr).await?;
    let local_addr = listener.local_addr()?;

    println!("audiorouter-dashboard listening on http://{local_addr}");
    let router = dashboard_router(state);
    axum::serve(listener, router).await?;
    Ok(())
}

struct Options {
    addr: SocketAddr,
    config_path: PathBuf,
}

impl Options {
    fn parse() -> anyhow::Result<Self> {
        let mut addr = std::env::var("AUDIOROUTER_DASHBOARD_ADDR")
            .unwrap_or_else(|_| DEFAULT_ADDR.to_string())
            .parse()?;
        let mut config_path = match std::env::var_os("AUDIOROUTER_CONFIG") {
            Some(path) => PathBuf::from(path),
            None => audiorouter_core::default_config_path()?,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--addr" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--addr requires a value"))?;
                    addr = value.parse()?;
                }
                "--config" => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--config requires a value"))?;
                    config_path = PathBuf::from(value);
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }

        Ok(Self { addr, config_path })
    }
}

fn print_help() {
    println!(
        "audiorouter-dashboard (serves embedded frontend + API)\n\nUSAGE:\n    cargo run --bin audiorouter-dashboard -- [--addr HOST:PORT] [--config PATH]\n\nENV:\n    AUDIOROUTER_DASHBOARD_ADDR   default: {DEFAULT_ADDR}\n    AUDIOROUTER_CONFIG           default: audiorouter platform config path\n    SKIP_DASHBOARD_BUILD=1       skip the `pnpm build` step in build.rs"
    );
}
