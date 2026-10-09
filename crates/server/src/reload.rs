//! Applying the config again while the daemon runs (`POST /api/v1/reload`, `ssp config reload`):
//! the file is read and checked first, and only a config the daemon can start with replaces the
//! running one. The daemon then stops its screens and the API and serves again with it, in the
//! same process (see [`crate::run`]).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::manager::Manager;
use crate::metrics::Metrics;
use crate::{drivers, schedule};

/// Reads the config file again, with the same command-line options as at start.
pub type Reload = Arc<dyn Fn() -> anyhow::Result<Config> + Send + Sync>;

/// Reads and checks the config, then hands it to the daemon to serve again with.
#[derive(Clone)]
pub struct Reloader {
    read: Reload,
    metrics: Metrics,
    restart: mpsc::Sender<Config>,
}

impl Reloader {
    /// A reloader that reads the config with `read` and sends it to `restart`.
    pub fn new(read: Reload, metrics: Metrics, restart: mpsc::Sender<Config>) -> Self {
        Self {
            read,
            metrics,
            restart,
        }
    }

    /// Reads the config and, if the daemon can start with it, has the daemon serve again with it.
    /// Returns the address the daemon is going to listen on.
    pub async fn reload(&self) -> anyhow::Result<SocketAddr> {
        let read = self.read.clone();
        let metrics = self.metrics.clone();
        let config = tokio::task::spawn_blocking(move || {
            let config = read()?;
            check(&config, &metrics)?;
            anyhow::Ok(config)
        })
        .await
        .context("cannot read the config")??;
        let listen = config.listen;
        self.restart
            .try_send(config)
            .map_err(|_| anyhow::anyhow!("the daemon is already reloading its config"))?;
        Ok(listen)
    }
}

/// Fails with what keeps the daemon from starting with `config`, as far as it can be told
/// without stopping: the settings, the drivers, what displays show at start and the schedule.
/// Whether the address to listen on is free only shows when the daemon listens.
pub fn check(config: &Config, metrics: &Metrics) -> anyhow::Result<()> {
    config.validate()?;
    let registry =
        drivers::registry(&config.drivers.selection()).context("invalid [drivers] config")?;
    Manager::new(registry, config, metrics.clone()).map_err(anyhow::Error::msg)?;
    schedule::Schedule::new(config, metrics).map_err(anyhow::Error::msg)?;
    Ok(())
}
