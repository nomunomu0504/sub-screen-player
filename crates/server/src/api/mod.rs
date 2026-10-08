//! HTTP + WebSocket API under `/api/v1`. See `docs/api.md`.

mod auth;
mod stream;
pub mod types;

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, middleware};
use ssp_core::Frame;
use tokio::sync::watch;

use crate::config::{ClockConfig, DashboardConfig};
use crate::manager::{DisplayState, LookupError, Manager};
use crate::metrics::{Metric, Metrics};
use crate::sources::{Clock, Content, Picture};
use types::{
    BrightnessRequest, CapabilitiesView, ClockRequest, DashboardRequest, DisplayView, ErrorBody,
    Health, ImageQuery, MetricUpdate, MetricView, PowerRequest, StatsView,
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
    shutdown: watch::Receiver<bool>,
}

impl AppState {
    /// `shutdown` turns `true` when the daemon is stopping; open streams then close.
    pub fn new(
        manager: Arc<Manager>,
        token: Option<String>,
        clock: ClockConfig,
        dashboard: DashboardConfig,
        metrics: Metrics,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        Self {
            manager,
            token: token.map(Into::into),
            clock,
            dashboard,
            metrics,
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
        .route(
            "/displays/{id}/image",
            post(show_image).layer(DefaultBodyLimit::max(MAX_IMAGE_BYTES)),
        )
        .route("/displays/{id}/brightness", post(brightness))
        .route("/displays/{id}/power", post(power))
        .route("/displays/{id}/clear", post(clear))
        .route("/displays/{id}/clock", post(clock))
        .route("/displays/{id}/dashboard", post(dashboard))
        .route("/displays/{id}/stop", post(stop))
        .route("/displays/{id}/stream", get(stream::stream))
        .route("/metrics", get(list_metrics))
        .route(
            "/metrics/{id}",
            put(set_metric).get(get_metric).delete(delete_metric),
        );
    Router::new()
        .nest("/api/v1", api)
        .layer(middleware::from_fn_with_state(state.clone(), auth::guard))
        .with_state(state)
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
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let fit = query.fit.unwrap_or_default().into();
    blocking(move || {
        let picture = Picture::decode(&body).map_err(ApiError::bad_request)?;
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
    request: Option<Json<ClockRequest>>,
) -> Result<StatusCode, ApiError> {
    let request = request.map(|Json(r)| r).unwrap_or_default();
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
    request: Option<Json<DashboardRequest>>,
) -> Result<StatusCode, ApiError> {
    let request = request.map(|Json(r)| r).unwrap_or_default();
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
