//! The sub-screen-player daemon (`ssp serve`).
//!
//! - [`manager`]: finds displays, keeps them open and remembers what each one shows.
//! - [`sources`]: built-in screens such as the clock and the dashboard.
//! - [`text`] and [`draw`]: text and shapes for the built-in screens.
//! - [`api`]: the HTTP + WebSocket API that clients (the CLI, scripts, apps) use.
//! - [`metrics`]: figures sent from outside, shown as dashboard panels.
//! - [`claude_code`]: the `claude-code` panel's reader of Claude Code's usage logs.
//! - [`web`]: headless Chrome for web pages (download, DevTools protocol).
//! - [`config`]: the TOML configuration.
//! - [`drivers`]: the list of device drivers compiled in.
#![warn(missing_docs)]

pub mod api;
pub mod claude_code;
pub mod config;
pub mod draw;
pub mod drivers;
pub mod manager;
pub mod metrics;
pub mod notify;
pub mod schedule;
pub mod sources;
pub mod text;
pub mod web;

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
    let registry =
        drivers::registry(&config.drivers.selection()).context("invalid [drivers] config")?;
    let metrics = metrics::Metrics::default();
    claude_code::register(&metrics, config.claude_code.clone());
    let manager =
        Arc::new(Manager::new(registry, &config, metrics.clone()).map_err(anyhow::Error::msg)?);
    let schedule = schedule::Schedule::new(&config, &metrics).map_err(anyhow::Error::msg)?;
    if let Some(schedule) = &schedule {
        manager.set_schedule(schedule.clone());
    }
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
    let scheduler = schedule.map(|s| s.spawn(manager.clone()));
    // Browsers a killed daemon left running; looking at all processes takes a moment.
    let _ = std::thread::Builder::new()
        .name("ssp-cleanup".into())
        .spawn(web::cdp::clean_up_after_others);
    let (stopping, stop_signal) = watch::channel(false);
    // Videos sent to the API wait in a folder of this daemon's own (one daemon per port).
    let uploads = std::env::temp_dir().join(format!("sub-screen-player-{}", config.listen.port()));
    let videos = sources::video::Videos::new(config.video.ffmpeg.clone(), uploads)
        .context("cannot create the folder for videos")?;
    let state = api::AppState::new(manager.clone(), &config, metrics, videos, stop_signal);
    let served = axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async move {
            shutdown.await;
            tracing::info!("shutting down");
            let _ = stopping.send(true);
        })
        .await;

    tokio::task::spawn_blocking(move || {
        if let Some(scheduler) = scheduler {
            scheduler.stop();
        }
        scanner.stop();
        manager.shutdown();
    })
    .await?;
    served.context("API server failed")
}
