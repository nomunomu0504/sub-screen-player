//! The sub-screen-player daemon (`ssp serve`).
//!
//! - [`manager`]: finds displays, keeps them open and remembers what each one shows.
//! - [`sources`]: built-in screens such as the clock and the dashboard.
//! - [`text`] and [`draw`]: text and shapes for the built-in screens.
//! - [`api`]: the HTTP + WebSocket API that clients (the CLI, scripts, apps) use.
//! - [`metrics`]: figures sent from outside, shown as dashboard panels.
//! - [`claude_code`]: the `claude-code` panel's reader of Claude Code's usage logs.
//! - [`web`]: headless Chrome for web pages (download, DevTools protocol).
//! - [`config`]: the TOML configuration, and [`reload`]: applying it again while running.
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
pub mod reload;
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
pub use reload::Reload;

/// How often the daemon looks for plugged and unplugged devices.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Runs the daemon until a future made by `shutdown` completes, then closes every display.
///
/// With `reload`, the daemon reads its config again on `POST /api/v1/reload` and serves again
/// with it; if the new config cannot start, it goes back to the one before. A new `shutdown`
/// future is made for every config.
pub async fn run<S, F>(config: Config, reload: Option<Reload>, shutdown: S) -> anyhow::Result<()>
where
    S: Fn() -> F,
    F: Future<Output = ()> + Send + 'static,
{
    let daemon = Daemon::new();
    let mut config = config;
    let mut previous = None;
    loop {
        match daemon
            .serve(config.clone(), reload.clone(), shutdown())
            .await
        {
            Ok(Stopped::Shutdown) => return Ok(()),
            Ok(Stopped::Reload(next)) => {
                daemon.set_reload_error(None);
                previous = Some(std::mem::replace(&mut config, *next));
            }
            Err(err) => match previous.take() {
                Some(old) => {
                    tracing::error!(
                        "cannot start with the new config, back to the one before: {err:#}"
                    );
                    daemon.set_reload_error(Some(format!("{err:#}")));
                    config = old;
                }
                None => return Err(err),
            },
        }
    }
}

/// Why [`Daemon::serve`] returned.
enum Stopped {
    /// `shutdown` completed; the displays got `[display] on_exit`.
    Shutdown,
    /// The config was reloaded: serve again with this one. The displays were left as they were.
    Reload(Box<Config>),
}

/// What the daemon keeps while its config is reloaded: the metrics sent to it, and the readers of
/// the built-in ones with their settings.
struct Daemon {
    metrics: metrics::Metrics,
    claude_code: claude_code::Settings,
    /// Why the last reload went back to the config before, for `GET /health`.
    reload_error: Arc<std::sync::Mutex<Option<String>>>,
}

impl Daemon {
    fn new() -> Self {
        let metrics = metrics::Metrics::default();
        let claude_code = claude_code::Settings::default();
        claude_code::register(&metrics, claude_code.clone());
        Self {
            metrics,
            claude_code,
            reload_error: Arc::default(),
        }
    }

    fn set_reload_error(&self, error: Option<String>) {
        *self.reload_error.lock().unwrap_or_else(|p| p.into_inner()) = error;
    }

    /// Serves with `config` until `shutdown` completes or the config is reloaded.
    async fn serve(
        &self,
        config: Config,
        reload: Option<Reload>,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> anyhow::Result<Stopped> {
        config.validate()?;
        let registry =
            drivers::registry(&config.drivers.selection()).context("invalid [drivers] config")?;
        let metrics = self.metrics.clone();
        self.claude_code.set(config.claude_code.clone());
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
        let uploads =
            std::env::temp_dir().join(format!("sub-screen-player-{}", config.listen.port()));
        let videos = sources::video::Videos::new(config.video.ffmpeg.clone(), uploads)
            .context("cannot create the folder for videos")?;
        let (restart, mut restarts) = tokio::sync::mpsc::channel(1);
        let reloader = reload.map(|read| reload::Reloader::new(read, metrics.clone(), restart));
        let state = api::AppState::new(manager.clone(), &config, metrics, videos, stop_signal)
            .with_reload(reloader, self.reload_error.clone());
        let (chosen, mut next) = tokio::sync::oneshot::channel();
        let served = axum::serve(listener, api::router(state))
            .with_graceful_shutdown(async move {
                tokio::select! {
                    () = shutdown => tracing::info!("shutting down"),
                    Some(config) = restarts.recv() => {
                        tracing::info!("reloading the config");
                        let _ = chosen.send(config);
                    }
                }
                let _ = stopping.send(true);
            })
            .await;

        let next = next.try_recv().ok();
        let reloading = next.is_some();
        tokio::task::spawn_blocking(move || {
            if let Some(scheduler) = scheduler {
                scheduler.stop();
            }
            scanner.stop();
            if reloading {
                manager.close();
            } else {
                manager.shutdown();
            }
        })
        .await?;
        served.context("API server failed")?;
        Ok(match next {
            Some(config) => Stopped::Reload(Box::new(config)),
            None => Stopped::Shutdown,
        })
    }
}
