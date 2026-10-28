//! `lethora-server [--config <path>]`: runs the sync server until Ctrl+C or
//! SIGTERM. The config file defaults to `./lethora-server.toml` (see
//! `lethora_server::config`); logging follows `RUST_LOG` (default `info`).

use std::path::PathBuf;
use std::process::ExitCode;

use lethora_server::Config;
use tracing::error;
use tracing_subscriber::EnvFilter;

fn usage() -> ExitCode {
    eprintln!("usage: lethora-server [--config <path>]");
    ExitCode::FAILURE
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let mut args = std::env::args().skip(1);
    let mut path = PathBuf::from("lethora-server.toml");
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--config", Some(value)) => path = PathBuf::from(value),
            _ => return usage(),
        }
    }
    let config = match Config::load(Some(&path), std::env::vars()) {
        Ok(config) => config,
        Err(e) => {
            error!("config {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let running = match lethora_server::start(config).await {
        Ok(running) => running,
        Err(e) => {
            error!("can't start: {e}");
            return ExitCode::FAILURE;
        }
    };
    wait_for_signal().await;
    running.stop().await;
    ExitCode::SUCCESS
}

/// Waits for Ctrl+C, or SIGTERM on Unix.
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
                return;
            }
            Err(e) => error!("can't listen for SIGTERM: {e}"),
        }
    }
    if let Err(e) = tokio::signal::ctrl_c().await {
        error!("can't listen for Ctrl+C: {e}");
        std::future::pending::<()>().await;
    }
}
