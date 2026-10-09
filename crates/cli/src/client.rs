//! Talks to a running daemon over its HTTP API.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ssp_server::api::types::{
    ChromeView, DisplayView, ErrorBody, Health, MetricView, ScheduleView,
};
use ureq::http::Response;

pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            agent,
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base)
    }

    fn auth(&self) -> Option<String> {
        self.token.as_ref().map(|t| format!("Bearer {t}"))
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let mut request = self.agent.get(self.url(path));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        let mut response = self.check(request.call())?;
        Ok(response.body_mut().read_json()?)
    }

    pub fn health(&self) -> Result<Health> {
        self.get("/health")
    }

    pub fn displays(&self) -> Result<Vec<DisplayView>> {
        self.get("/displays")
    }

    pub fn schedule(&self) -> Result<ScheduleView> {
        self.get("/schedule")
    }

    pub fn metrics(&self) -> Result<Vec<MetricView>> {
        self.get("/metrics")
    }

    pub fn chrome(&self) -> Result<ChromeView> {
        self.get("/web/chrome")
    }

    /// Has the daemon download headless Chrome; waits as long as that takes.
    pub fn install_chrome(&self) -> Result<ChromeView> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30 * 60)))
            .build()
            .into();
        let mut request = agent.post(self.url("/web/chrome"));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        let mut response = self.check(request.send_empty())?;
        Ok(response.body_mut().read_json()?)
    }

    pub fn put_json(&self, path: &str, body: &impl Serialize) -> Result<()> {
        let mut request = self.agent.put(self.url(path));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        self.check(request.send_json(body)).map(drop)
    }

    pub fn delete(&self, path: &str) -> Result<()> {
        let mut request = self.agent.delete(self.url(path));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        self.check(request.call()).map(drop)
    }

    pub fn post_json(&self, path: &str, body: &impl Serialize) -> Result<()> {
        let mut request = self.agent.post(self.url(path));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        self.check(request.send_json(body)).map(drop)
    }

    pub fn post_empty(&self, path: &str) -> Result<()> {
        let mut request = self.agent.post(self.url(path));
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        self.check(request.send_empty()).map(drop)
    }

    /// Sends a file as the body, without reading it into memory first (videos can be large).
    pub fn post_file(&self, path: &str, file: &std::path::Path) -> Result<()> {
        let body =
            std::fs::File::open(file).with_context(|| format!("cannot read {}", file.display()))?;
        let mut request = self
            .agent
            .post(self.url(path))
            .header("Content-Type", "application/octet-stream");
        if let Some(auth) = self.auth() {
            request = request.header("Authorization", auth);
        }
        self.check(request.send(body)).map(drop)
    }

    /// Turns transport failures and error statuses into readable errors.
    fn check(
        &self,
        result: Result<Response<ureq::Body>, ureq::Error>,
    ) -> Result<Response<ureq::Body>> {
        let mut response = result.map_err(|err| match err {
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
                anyhow::Error::new(Unreachable {
                    base: self.base.clone(),
                    cause: err.to_string(),
                })
            }
            other => anyhow::Error::new(other).context("request to the daemon failed"),
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.body_mut().read_to_string().unwrap_or_default();
        let message = serde_json::from_str::<ErrorBody>(&body).map_or(body, |e| e.error);
        Err(anyhow!("{message} (HTTP {})", status.as_u16()))
            .context("the daemon refused the request")
    }
}

/// The daemon is not running (or not at this address).
#[derive(Debug)]
pub struct Unreachable {
    base: String,
    cause: String,
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot reach the daemon at {} ({}).\n\
             Start it with `ssp serve`, or `ssp service install` to run it at login.",
            self.base, self.cause
        )
    }
}

impl std::error::Error for Unreachable {}

pub fn is_unreachable(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Unreachable>().is_some()
}
