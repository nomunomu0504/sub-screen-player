//! The sub-screen-player daemon (`ssp serve`).
//!
//! - [`manager`]: finds displays, keeps them open and remembers what each one shows.
//! - [`sources`]: built-in screens such as the clock.
//! - [`api`]: the HTTP + WebSocket API that clients (the CLI, scripts, apps) use.
//! - [`config`]: the TOML configuration.
//! - [`drivers`]: the list of device drivers compiled in.
#![warn(missing_docs)]

pub mod api;
pub mod config;
pub mod drivers;
pub mod manager;
pub mod sources;
pub mod text;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::sync::watch;

pub use config::Config;
use manager::Manager;

/// How often the daemon looks for plugged and unplugged devices.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Runs the daemon until `shutdown` completes, then closes every display.
pub async fn serve(
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    config.validate()?;
    let manager = Arc::new(Manager::new(drivers::registry(), &config).map_err(anyhow::Error::msg)?);
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| {
            format!(
                "cannot listen on {} (is the daemon already running?)",
                config.listen
            )
        })?;
    tracing::info!("API listening on http://{}", listener.local_addr()?);

    let scanner = manager.spawn_scanner(SCAN_INTERVAL);
    let (stopping, stop_signal) = watch::channel(false);
    let state = api::AppState::new(
        manager.clone(),
        config.token.clone(),
        config.clock.clone(),
        stop_signal,
    );
    let served = axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move {
            shutdown.await;
            tracing::info!("shutting down");
            let _ = stopping.send(true);
        })
        .await;

    tokio::task::spawn_blocking(move || {
        scanner.stop();
        manager.shutdown();
    })
    .await?;
    served.context("API server failed")
}
