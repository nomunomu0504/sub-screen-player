//! HTTP + WebSocket API under `/api/v1`. See `docs/api.md`.

mod auth;
mod stream;
#[cfg(test)]
mod tests;
pub mod types;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, BodyDataStream};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, middleware};
use futures_util::StreamExt;
use ssp_core::{Fit, Frame};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use crate::config::{ClockConfig, Config, DashboardConfig};
use crate::manager::{DisplayState, LookupError, Manager};
use crate::metrics::{Metric, Metrics};
use crate::sources::stats::Stats;
use crate::sources::video::{self, MAX_VIDEO_BYTES, VideoFile, Videos};
use crate::sources::web::WebPage;
use crate::sources::{Clock, Content, Picture};
use crate::web::Web;
use types::{
    BrightnessRequest, CapabilitiesView, ChromeView, ClockRequest, DashboardRequest, DisplayView,
    ErrorBody, Health, ImageQuery, MetricUpdate, MetricView, PowerRequest, StatsView, SystemView,
    WebRequest,
};

/// Largest request body accepted for images.
const MAX_IMAGE_BYTES: usize = 64 << 20;

/// Shared state of all handlers.
#[derive(Clone)]
pub struct AppState {
    manager: Arc<Manager>,
    token: Option<Arc<str>>,
    clock: ClockConfig,
    dashboard: DashboardConfig,
    metrics: Metrics,
    videos: Arc<Videos>,
    web: Web,
    /// Measures `GET /system`.
    system: Arc<Mutex<Stats>>,
    shutdown: watch::Receiver<bool>,
}

impl AppState {
    /// The API of `manager` with the token, defaults and browser settings of `config`.
    /// `shutdown` turns `true` when the daemon is stopping; open streams then close.
    pub fn new(
        manager: Arc<Manager>,
        config: &Config,
        metrics: Metrics,
        videos: Videos,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            manager,
            token: config.token.clone().map(Into::into),
            clock: config.clock.clone(),
            dashboard: config.dashboard.clone(),
            metrics,
            videos: Arc::new(videos),
            web: Web::new(&config.web).with_api(config.listen),
            system: Arc::new(Mutex::new(Stats::new())),
            shutdown,
        }
    }
}

/// All routes.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/displays", get(list))
        .route("/displays/{id}", get(one))
        .route("/displays/{id}/image", post(show_image))
        .route("/displays/{id}/brightness", post(brightness))
        .route("/displays/{id}/power", post(power))
        .route("/displays/{id}/clear", post(clear))
        .route("/displays/{id}/clock", post(clock))
        .route("/displays/{id}/dashboard", post(dashboard))
        .route("/displays/{id}/stop", post(stop))
        .route("/displays/{id}/stream", get(stream::stream))
        .route("/displays/{id}/web", post(show_web))
        .route("/web/chrome", get(chrome_status).post(install_chrome))
        .route("/system", get(system))
        .route("/metrics", get(list_metrics))
        .route(
            "/metrics/{id}",
            put(set_metric).get(get_metric).delete(delete_metric),
        );
    Router::new()
        .nest("/api/v1", api)
        .layer(middleware::from_fn_with_state(state.clone(), auth::guard))
        .layer(middleware::map_response(json_errors))
        .with_state(state)
}

/// Turns the error answers axum makes itself (malformed JSON, a bad query, an unknown path),
/// which are plain text, into the `{"error": "..."}` body of every other error.
async fn json_errors(response: Response) -> Response {
    let status = response.status();
    let json = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(b"application/json"));
    if !(status.is_client_error() || status.is_server_error()) || json {
        return response;
    }
    let message = match axum::body::to_bytes(response.into_body(), 64 << 10).await {
        Ok(text) if !text.is_empty() => String::from_utf8_lossy(&text).into_owned(),
        _ => status.canonical_reason().unwrap_or("error").to_owned(),
    };
    ApiError::new(status, message).into_response()
}

/// An optional JSON body, read whatever its `Content-Type` says (scripts often send none). An
/// empty body is `None`.
fn optional_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Option<T>, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(body)
        .map(Some)
        .map_err(|e| ApiError::bad_request(format!("invalid request: {e}")))
}

/// An error response with a JSON body.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

impl From<ssp_core::Error> for ApiError {
    fn from(err: ssp_core::Error) -> Self {
        use ssp_core::Error as E;
        let status = match &err {
            E::InvalidArgument(_) | E::Image(_) => StatusCode::BAD_REQUEST,
            E::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            E::Transport(_) | E::Disconnected | E::Closed => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self::new(status, err.to_string())
    }
}

impl From<LookupError> for ApiError {
    fn from(err: LookupError) -> Self {
        let status = match err {
            LookupError::NotFound(_) => StatusCode::NOT_FOUND,
            LookupError::NotConnected(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self::new(status, err.to_string())
    }
}

/// Runs blocking device work off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("request failed: {e}"),
        )
    })?
}

fn view(state: DisplayState) -> DisplayView {
    let caps = &state.info.capabilities;
    DisplayView {
        driver: state.info.driver.to_string(),
        model: state.info.model.clone(),
        serial: state.info.serial.clone(),
        firmware: state.info.firmware.clone(),
        connected: state.connected,
        width: state.info.panel.width,
        height: state.info.panel.height,
        content: state.content.to_string(),
        capabilities: CapabilitiesView {
            live_frames: caps.live_frames,
            saved_frames: caps.saved_frames,
            brightness: caps.brightness,
            power: caps.power,
            clear: caps.clear,
            max_fps: caps.max_fps,
        },
        stats: state.stats.map(|s| StatsView {
            submitted: s.submitted,
            shown: s.shown,
            partial: s.partial,
            dropped: s.dropped,
            duplicates: s.duplicates,
            last_encode_ms: s.last_encode.as_secs_f64() * 1000.0,
            last_send_ms: s.last_send.as_secs_f64() * 1000.0,
            last_bytes: s.last_bytes,
            quality: s.quality,
        }),
        id: state.id,
    }
}

async fn health(State(app): State<AppState>) -> Json<Health> {
    Json(Health {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        drivers: Some(
            app.manager
                .drivers()
                .into_iter()
                .map(String::from)
                .collect(),
        ),
    })
}

async fn list(State(app): State<AppState>) -> Json<Vec<DisplayView>> {
    Json(app.manager.displays().into_iter().map(view).collect())
}

async fn one(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<DisplayView>, ApiError> {
    Ok(Json(view(app.manager.display(&id)?)))
}

async fn show_image(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ImageQuery>,
    body: Body,
) -> Result<StatusCode, ApiError> {
    let fit = query.fit.unwrap_or_default().into();
    // The first bytes tell images from videos; videos go to a file instead of memory.
    let mut chunks = body.into_data_stream();
    let mut bytes = Vec::new();
    while bytes.len() < 256 {
        match chunks.next().await {
            Some(chunk) => bytes.extend_from_slice(&chunk.map_err(unreadable)?),
            None => break,
        }
    }
    if video::is_video(&bytes) && image::guess_format(&bytes).is_err() {
        return show_video(app, id, query, fit, bytes, chunks).await;
    }
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(unreadable)?;
        if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
            return Err(too_large(MAX_IMAGE_BYTES as u64, "an image"));
        }
        bytes.extend_from_slice(&chunk);
    }
    blocking(move || {
        let picture = Picture::decode(&bytes).map_err(ApiError::bad_request)?;
        if query.persist {
            // The device holds one picture: an animation stores its first frame.
            let device = app.manager.device(&id)?;
            let panel = device.presenter.info().panel;
            app.manager.set_content(&id, Content::Nothing)?;
            device
                .presenter
                .save(Frame::fit(picture.first(), panel.width, panel.height, fit))?;
        }
        app.manager.set_content(&id, picture.into_content(fit))?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Receives a video whose first bytes are `head` into a file and plays it.
async fn show_video(
    app: AppState,
    id: String,
    query: ImageQuery,
    fit: Fit,
    head: Vec<u8>,
    mut chunks: BodyDataStream,
) -> Result<StatusCode, ApiError> {
    let refused = if query.persist {
        Some(ApiError::bad_request(
            "a video cannot be stored on the display (persist); store a picture instead",
        ))
    } else {
        app.videos
            .ffmpeg()
            .map_err(|e| ApiError::new(StatusCode::NOT_IMPLEMENTED, e))
            .err()
    };
    if let Some(err) = refused {
        // Read the rest first: a client still sending would see a broken connection instead.
        while let Some(Ok(_)) = chunks.next().await {}
        return Err(err);
    }
    let ffmpeg = app.videos.ffmpeg().map_err(ApiError::bad_request)?;
    let path = app.videos.file();
    if let Err(err) = receive(&path, head, &mut chunks).await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(err);
    }
    blocking(move || {
        let video = VideoFile::open(path, ffmpeg, true).map_err(ApiError::bad_request)?;
        app.manager.set_content(
            &id,
            Content::Video {
                video: Arc::new(video),
                fit,
            },
        )?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Writes `head` and the rest of the body to `path`.
async fn receive(
    path: &std::path::Path,
    head: Vec<u8>,
    chunks: &mut BodyDataStream,
) -> Result<(), ApiError> {
    let failed = |e: std::io::Error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot store the video: {e}"),
        )
    };
    let mut file = tokio::fs::File::create(path).await.map_err(failed)?;
    let mut size = head.len() as u64;
    file.write_all(&head).await.map_err(failed)?;
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(unreadable)?;
        size += chunk.len() as u64;
        if size > MAX_VIDEO_BYTES {
            return Err(too_large(MAX_VIDEO_BYTES, "a video"));
        }
        file.write_all(&chunk).await.map_err(failed)?;
    }
    file.flush().await.map_err(failed)
}

fn unreadable(err: axum::Error) -> ApiError {
    ApiError::bad_request(format!("cannot read the request body: {err}"))
}

fn too_large(limit: u64, what: &str) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("{what} may be at most {} MiB", limit >> 20),
    )
}

async fn show_web(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<WebRequest>,
) -> Result<StatusCode, ApiError> {
    let page = WebPage::new(&request.url, request.reload).map_err(ApiError::bad_request)?;
    blocking(move || {
        // Find (or download, if allowed) the browser now, so a missing one is reported here
        // rather than only on the display.
        app.web
            .ensure()
            .map_err(|e| ApiError::new(StatusCode::CONFLICT, e))?;
        app.manager.set_content(
            &id,
            Content::Web {
                page,
                web: app.web.clone(),
            },
        )?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn chrome_status(State(app): State<AppState>) -> Result<Json<ChromeView>, ApiError> {
    blocking(move || Ok(Json(app.web.status()))).await
}

async fn install_chrome(State(app): State<AppState>) -> Result<Json<ChromeView>, ApiError> {
    blocking(move || {
        app.web
            .install()
            .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e))?;
        Ok(Json(app.web.status()))
    })
    .await
}

async fn brightness(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<BrightnessRequest>,
) -> Result<StatusCode, ApiError> {
    blocking(move || {
        app.manager
            .device(&id)?
            .presenter
            .set_brightness(request.percent)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn power(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PowerRequest>,
) -> Result<StatusCode, ApiError> {
    blocking(move || {
        let device = app.manager.device(&id)?;
        if request.on {
            device.presenter.wake()
        } else {
            device.presenter.sleep()
        }?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn clear(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    blocking(move || {
        let device = app.manager.device(&id)?;
        app.manager.set_content(&id, Content::Nothing)?;
        device.presenter.clear()?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn clock(
    State(app): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let request: ClockRequest = optional_json(&body)?.unwrap_or_default();
    let mut config = app.clock.clone();
    if let Some(seconds) = request.seconds {
        config.seconds = seconds;
        config.time_format = None;
    }
    config.time_format = request.time_format.or(config.time_format);
    config.date_format = request.date_format.unwrap_or(config.date_format);
    config.weekdays = request.weekdays.unwrap_or(config.weekdays);
    config.color = request.color.unwrap_or(config.color);
    config.background = request.background.unwrap_or(config.background);
    for color in [&config.color, &config.background] {
        if crate::config::parse_color(color).is_none() {
            return Err(ApiError::bad_request(format!(
                "{color:?} is not a #RRGGBB color"
            )));
        }
    }
    Clock::validate(&config).map_err(ApiError::bad_request)?;
    blocking(move || {
        app.manager.set_content(&id, Content::Clock(config))?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn dashboard(
    State(app): State<AppState>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<StatusCode, ApiError> {
    let request: DashboardRequest = optional_json(&body)?.unwrap_or_default();
    let mut config = app.dashboard.clone();
    config.widgets = request.widgets.unwrap_or(config.widgets);
    config.color = request.color.unwrap_or(config.color);
    config.accent = request.accent.unwrap_or(config.accent);
    config.background = request.background.unwrap_or(config.background);
    config.validate().map_err(ApiError::bad_request)?;
    let clock = app.clock.clone();
    let metrics = app.metrics.clone();
    blocking(move || {
        app.manager
            .set_content(&id, Content::Dashboard(config, clock, metrics))?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn stop(State(app): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    blocking(move || {
        app.manager.set_content(&id, Content::Nothing)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn system(State(app): State<AppState>) -> Result<Json<SystemView>, ApiError> {
    let now = tokio::task::spawn_blocking(move || {
        let mut stats = app.system.lock().unwrap_or_else(|p| p.into_inner());
        stats.refresh_or_wait();
        stats.now.clone()
    })
    .await
    .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(SystemView {
        cpu_percent: now.cpu_percent,
        cpu_count: now.cpu_count,
        load: now.load,
        memory_used: now.memory_used,
        memory_total: now.memory_total,
        rx_per_sec: now.rx_per_sec,
        tx_per_sec: now.tx_per_sec,
        disk_used: now.disk.map(|(used, _)| used),
        disk_total: now.disk.map(|(_, total)| total),
    }))
}

async fn list_metrics(State(app): State<AppState>) -> Json<Vec<MetricView>> {
    Json(
        app.metrics
            .list()
            .into_iter()
            .map(|(id, metric)| metric_view(id, &metric))
            .collect(),
    )
}

async fn get_metric(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MetricView>, ApiError> {
    // A built-in metric (`claude-code`) starts being gathered when it is first asked for, as
    // when a dashboard first shows its panel; until the first figures are in, it is not found.
    app.metrics.activate(&id);
    let metric = app
        .metrics
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("no metric {id:?}")))?;
    Ok(Json(metric_view(id, &metric)))
}

async fn set_metric(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(update): Json<MetricUpdate>,
) -> Result<StatusCode, ApiError> {
    app.metrics
        .set(&id, update)
        .map_err(ApiError::bad_request)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_metric(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if app.metrics.remove(&id) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no metric {id:?}")))
    }
}

fn metric_view(id: String, metric: &Metric) -> MetricView {
    MetricView {
        label: metric.label.clone().unwrap_or_else(|| id.clone()),
        id,
        value: metric.value,
        text: metric.text.clone(),
        unit: metric.unit.clone(),
        detail: metric.detail.clone(),
        max: metric.max,
        ttl: metric.ttl.as_secs(),
        updated: metric.updated_at.to_string(),
        age: metric.age().as_secs(),
        stale: metric.is_stale(),
        history: metric.history.iter().copied().collect(),
    }
}
