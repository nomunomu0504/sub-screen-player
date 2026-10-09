//! The API driven through its router, as clients use it, with a fake display attached. Nothing
//! here touches USB devices, the network or the user's files.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderName, Request, StatusCode, header};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame as ImageFrame, ImageFormat, Rgba, RgbaImage};
use serde::de::DeserializeOwned;
use ssp_core::Registry;
use ssp_core::testing::{Call, CallLog, FakeDisplay};
use tokio::sync::watch;
use tower::ServiceExt;

use super::types::{DisplayView, ErrorBody, Health, MetricView, SystemView};
use super::{AppState, router};
use crate::config::{Config, StartupConfig, StartupShow};
use crate::manager::Manager;
use crate::metrics::Metrics;
use crate::sources::video::{self, Videos};
use crate::web::page_token::PageToken;

/// The id of the fake display.
const DISPLAY: &str = "fake-0001";
const TOKEN: &str = "0123456789abcdef";
/// The start of an MP4 file: enough for the API to take a body for a video.
const MP4_HEAD: &[u8] = b"\0\0\0\x20ftypisom\0\0\x02\0isomiso2avc1mp41";

/// The API of a daemon with one fake display. Uploaded videos go to a fresh temporary folder,
/// removed when the test ends.
struct Api {
    router: Router,
    manager: Arc<Manager>,
    log: CallLog,
    dir: PathBuf,
}

impl Api {
    fn new(config: Config) -> Self {
        // Tests run in parallel in one process.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ssp-api-{}-{n}", std::process::id()));
        let videos = Videos::new(config.video.ffmpeg.clone(), dir.join("uploads")).unwrap();
        let metrics = Metrics::default();
        let manager = Arc::new(Manager::new(Registry::new(), &config, metrics.clone()).unwrap());
        let (display, log) = FakeDisplay::new();
        manager.attach(Box::new(display), c"fake".to_owned());
        // Only WebSocket streams watch for the daemon stopping, and these tests open none.
        let (_, shutdown) = watch::channel(false);
        let state = AppState::new(manager.clone(), &config, metrics, videos, shutdown);
        Self {
            router: router(state),
            manager,
            log,
            dir,
        }
    }

    /// The status and body of the answer to `request`.
    async fn send(&self, request: Request<Body>) -> (StatusCode, String) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    /// `GET uri`, which must succeed.
    async fn get<T: DeserializeOwned>(&self, uri: &str) -> T {
        let (status, body) = self.send(request("GET", uri)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        serde_json::from_str(&body).unwrap()
    }

    /// What the fake display shows.
    async fn content(&self) -> String {
        let display: DisplayView = self.get(&format!("/api/v1/displays/{DISPLAY}")).await;
        display.content
    }

    /// The videos the daemon keeps.
    fn uploads(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.dir.join("uploads"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect()
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        self.manager.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The default config, except that the display starts with nothing on it, so whatever the fake
/// display gets comes from the requests of the test.
fn config() -> Config {
    Config {
        startup: StartupConfig {
            show: StartupShow::Nothing,
            ..StartupConfig::default()
        },
        ..Config::default()
    }
}

fn request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

fn json_request(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

/// A POST of `body` without a content type, as files are sent.
fn post(uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .body(body.into())
        .unwrap()
}

fn with_header(mut request: Request<Body>, name: HeaderName, value: &str) -> Request<Body> {
    request.headers_mut().insert(name, value.parse().unwrap());
    request
}

/// The message of an error answer of the API.
fn error(body: &str) -> String {
    serde_json::from_str::<ErrorBody>(body)
        .unwrap_or_else(|e| panic!("not an error body ({e}): {body}"))
        .error
}

/// A small PNG file.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255]))
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .unwrap();
    bytes
}

/// A GIF of two frames, red then blue.
fn gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            let image = RgbaImage::from_pixel(8, 4, Rgba(color));
            let delay = Delay::from_numer_denom_ms(100, 1);
            encoder
                .encode_frame(ImageFrame::from_parts(image, 0, 0, delay))
                .unwrap();
        }
    }
    bytes
}

fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[tokio::test]
async fn reports_health() {
    let api = Api::new(config());
    let health: Health = api.get("/api/v1/health").await;
    assert_eq!(health.status, "ok");
    assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
    // The registry is empty: the fake display was attached by hand.
    assert_eq!(health.drivers, Some(Vec::new()));
}

#[tokio::test]
async fn lists_displays() {
    let api = Api::new(config());
    let displays: Vec<DisplayView> = api.get("/api/v1/displays").await;
    assert_eq!(displays.len(), 1);
    let display = &displays[0];
    assert_eq!(display.id, DISPLAY);
    assert_eq!(
        (display.driver.as_str(), display.serial.as_str()),
        ("fake", "0001")
    );
    assert_eq!((display.width, display.height), (1920, 462));
    assert_eq!(display.content, "nothing");
    assert!(display.connected);
    assert!(display.capabilities.brightness);
    assert!(display.stats.is_some());

    let default: DisplayView = api.get("/api/v1/displays/default").await;
    assert_eq!(default.id, DISPLAY);
    let (status, body) = api.send(request("GET", "/api/v1/displays/nope")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(error(&body).contains("nope"), "{body}");
}

#[tokio::test]
async fn keeps_metrics() {
    let api = Api::new(config());
    for (uri, body) in [
        (
            "/api/v1/metrics/ci",
            r#"{"label": "CI", "value": 3, "unit": "failed", "detail": "main"}"#,
        ),
        ("/api/v1/metrics/ci", r#"{"value": 1}"#),
        ("/api/v1/metrics/build", r#"{"text": "passing"}"#),
    ] {
        let (status, body) = api.send(json_request("PUT", uri, body)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }

    let ci: MetricView = api.get("/api/v1/metrics/ci").await;
    assert_eq!((ci.id.as_str(), ci.label.as_str()), ("ci", "CI"));
    assert_eq!((ci.value, ci.text.as_deref()), (Some(1.0), None));
    assert_eq!((ci.unit.as_str(), ci.detail.as_str()), ("failed", "main"));
    assert_eq!(ci.history, [3.0, 1.0]);
    assert_eq!((ci.max, ci.ttl, ci.stale), (None, 300, false));
    assert!(
        ci.updated.parse::<jiff::Timestamp>().is_ok(),
        "{}",
        ci.updated
    );

    // Sorted by id; the label defaults to the id.
    let all: Vec<MetricView> = api.get("/api/v1/metrics").await;
    let ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["build", "ci"]);
    assert_eq!(all[0].label, "build");
    assert_eq!(
        (all[0].value, all[0].text.as_deref()),
        (None, Some("passing"))
    );

    let (status, _) = api.send(request("DELETE", "/api/v1/metrics/ci")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for method in ["DELETE", "GET"] {
        let (status, body) = api.send(request(method, "/api/v1/metrics/ci")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        assert!(error(&body).contains("no metric"), "{body}");
    }
}

#[tokio::test]
async fn rejects_bad_metrics() {
    let api = Api::new(config());
    for (uri, body, message) in [
        ("/api/v1/metrics/CI", r#"{"value": 1}"#, "invalid metric id"),
        (
            "/api/v1/metrics/ci",
            r#"{"value": 1, "text": "up"}"#,
            "either value or text",
        ),
        (
            "/api/v1/metrics/ci",
            r#"{"label": "CI"}"#,
            "does not exist yet",
        ),
        ("/api/v1/metrics/ci", r#"{"value": 1, "ttl": 0}"#, "ttl"),
    ] {
        let (status, body) = api.send(json_request("PUT", uri, body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(error(&body).contains(message), "{body}");
    }

    // Bodies that are not a metric update are refused by axum's `Json` extractor; the answer is
    // still a JSON error.
    let typo = json_request("PUT", "/api/v1/metrics/ci", r#"{"vaule": 1}"#);
    let (status, body) = api.send(typo).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error(&body).contains("unknown field `vaule`"), "{body}");
    let cut = json_request("PUT", "/api/v1/metrics/ci", r#"{"value": 1"#);
    let (status, body) = api.send(cut).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!error(&body).is_empty(), "{body}");

    let all: Vec<MetricView> = api.get("/api/v1/metrics").await;
    assert!(all.is_empty());
}

#[tokio::test]
async fn shows_images_and_animations() {
    let api = Api::new(config());
    let uri = "/api/v1/displays/fake-0001/image";
    let (status, body) = api.send(post(uri, png())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(api.content().await, "image");
    wait_until(|| api.log.calls().iter().any(|c| matches!(c, Call::Show(_))));

    let (status, body) = api.send(post(uri, gif())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(api.content().await, "animation");

    // `persist` also stores the picture on the device.
    let saved = || api.log.calls().iter().any(|c| matches!(c, Call::Save(_)));
    assert!(!saved());
    let uri = "/api/v1/displays/fake-0001/image?fit=cover&persist=true";
    let (status, body) = api.send(post(uri, png())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(saved());
    assert_eq!(api.content().await, "image");
}

#[tokio::test]
async fn rejects_bad_images() {
    let api = Api::new(config());
    let garbage = post("/api/v1/displays/fake-0001/image", "not an image");
    let (status, body) = api.send(garbage).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("cannot decode image"), "{body}");

    let (status, body) = api.send(post("/api/v1/displays/nope/image", png())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(error(&body).contains("nope"), "{body}");

    // Refused by axum's `Query` extractor, as a JSON error too.
    let sideways = post("/api/v1/displays/fake-0001/image?fit=sideways", png());
    let (status, body) = api.send(sideways).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("fit"), "{body}");
    assert_eq!(api.content().await, "nothing");
}

#[tokio::test]
async fn refuses_videos_without_ffmpeg_and_to_persist() {
    let mut config = config();
    config.video.ffmpeg = Some(std::env::temp_dir().join("ssp-api-no-ffmpeg"));
    let api = Api::new(config);
    let uri = "/api/v1/displays/fake-0001/image";
    let (status, body) = api.send(post(uri, MP4_HEAD)).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(error(&body).contains("does not exist"), "{body}");

    let uri = "/api/v1/displays/fake-0001/image?persist=true";
    let (status, body) = api.send(post(uri, MP4_HEAD)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("persist"), "{body}");
    assert!(api.uploads().is_empty());
    assert_eq!(api.content().await, "nothing");
}

#[tokio::test]
async fn plays_an_uploaded_video_until_replaced() {
    let Ok(ffmpeg) = video::find_ffmpeg(None) else {
        eprintln!("skipped: ffmpeg is not installed");
        return;
    };
    let api = Api::new(config());
    let clip = api.dir.join("clip.mp4");
    let made = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi"])
        .args([
            "-i",
            "testsrc2=size=64x36:rate=10",
            "-t",
            "1",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success());

    let uri = "/api/v1/displays/fake-0001/image";
    let (status, body) = api.send(post(uri, std::fs::read(&clip).unwrap())).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(api.content().await, "video");
    assert_eq!(api.uploads().len(), 1);

    // The file is deleted once nothing plays it.
    let stop = request("POST", "/api/v1/displays/fake-0001/stop");
    assert_eq!(api.send(stop).await.0, StatusCode::NO_CONTENT);
    assert!(api.uploads().is_empty());

    // A file ffmpeg cannot play is refused, and deleted too.
    let mut broken = MP4_HEAD.to_vec();
    broken.resize(4096, 0);
    let (status, body) = api.send(post(uri, broken)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("ffmpeg cannot play"), "{body}");
    assert!(api.uploads().is_empty());
    assert_eq!(api.content().await, "nothing");
}

#[tokio::test]
async fn refuses_web_pages_it_cannot_show() {
    let mut config = config();
    // A browser that is not there, so nothing is started or downloaded.
    config.web.chrome = Some(std::env::temp_dir().join("ssp-api-no-chrome"));
    let api = Api::new(config);
    let uri = "/api/v1/displays/fake-0001/web";
    let script = json_request("POST", uri, r#"{"url": "javascript:alert(1)"}"#);
    let (status, body) = api.send(script).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("only http, https and file"), "{body}");

    let page = json_request("POST", uri, r#"{"url": "https://example.com/"}"#);
    let (status, body) = api.send(page).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(error(&body).contains("does not exist"), "{body}");
    assert_eq!(api.content().await, "nothing");
}

#[tokio::test]
async fn shows_a_dashboard() {
    let api = Api::new(config());
    let uri = "/api/v1/displays/fake-0001/dashboard";
    let widgets = r#"{"widgets": ["clock", "metric:ci", "claude-code"]}"#;
    let (status, body) = api.send(json_request("POST", uri, widgets)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(api.content().await, "dashboard");

    // The body is optional.
    let (status, body) = api.send(request("POST", uri)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // An unknown widget is refused.
    let gpu = json_request("POST", uri, r#"{"widgets": ["gpu"]}"#);
    let (status, body) = api.send(gpu).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("unknown widget"), "{body}");
    let green = json_request("POST", uri, r#"{"accent": "green"}"#);
    let (status, body) = api.send(green).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("#RRGGBB"), "{body}");

    // A body is read without `Content-Type: application/json` too, and misspelled fields are
    // refused rather than ignored.
    let (status, body) = api.send(post(uri, r#"{"widgets": ["gpu"]}"#)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("unknown widget"), "{body}");
    let (status, body) = api.send(post(uri, r#"{"widgets": ["cpu"]}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let typo = json_request("POST", uri, r#"{"widget": ["cpu"]}"#);
    let (status, body) = api.send(typo).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("unknown field `widget`"), "{body}");

    // Unknown paths answer with a JSON error as well.
    let (status, body) = api.send(request("GET", "/api/v1/nothing-here")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(!error(&body).is_empty(), "{body}");
}

#[tokio::test]
async fn shows_a_clock_and_stops() {
    let api = Api::new(config());
    let uri = "/api/v1/displays/fake-0001/clock";
    let no_seconds = json_request("POST", uri, r#"{"seconds": false}"#);
    let (status, body) = api.send(no_seconds).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(api.content().await, "clock");
    let red = json_request("POST", uri, r#"{"color": "red"}"#);
    let (status, body) = api.send(red).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("#RRGGBB"), "{body}");

    let stop = request("POST", "/api/v1/displays/fake-0001/stop");
    assert_eq!(api.send(stop).await.0, StatusCode::NO_CONTENT);
    assert_eq!(api.content().await, "nothing");
}

#[tokio::test]
async fn controls_the_display() {
    let api = Api::new(config());
    for (uri, body) in [
        (
            "/api/v1/displays/fake-0001/brightness",
            r#"{"percent": 50}"#,
        ),
        ("/api/v1/displays/fake-0001/power", r#"{"on": false}"#),
        ("/api/v1/displays/fake-0001/power", r#"{"on": true}"#),
    ] {
        let (status, body) = api.send(json_request("POST", uri, body)).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let clear = request("POST", "/api/v1/displays/fake-0001/clear");
    assert_eq!(api.send(clear).await.0, StatusCode::NO_CONTENT);
    assert_eq!(
        api.log.take(),
        [Call::Brightness(50), Call::Sleep, Call::Wake, Call::Clear]
    );

    let uri = "/api/v1/displays/fake-0001/brightness";
    let (status, body) = api
        .send(json_request("POST", uri, r#"{"percent": 101}"#))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(error(&body).contains("over 100"), "{body}");
    assert!(api.log.take().is_empty());
}

#[tokio::test]
async fn needs_the_token_when_one_is_set() {
    let api = Api::new(Config {
        token: Some(TOKEN.into()),
        ..config()
    });
    let health = || request("GET", "/api/v1/health");
    let bearer = format!("Bearer {TOKEN}");

    let (status, body) = api.send(health()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(error(&body).contains("token"), "{body}");
    let wrong = with_header(health(), header::AUTHORIZATION, "Bearer 0123456789abcdeX");
    assert_eq!(api.send(wrong).await.0, StatusCode::UNAUTHORIZED);
    let right = with_header(health(), header::AUTHORIZATION, &bearer);
    assert_eq!(api.send(right).await.0, StatusCode::OK);
    // For browser WebSockets, which cannot set headers.
    let query = request("GET", &format!("/api/v1/health?token={TOKEN}"));
    assert_eq!(api.send(query).await.0, StatusCode::OK);
    // The token replaces the origin check: a page that has it may use the API.
    let page = with_header(health(), header::ORIGIN, "https://evil.example");
    let page = with_header(page, header::AUTHORIZATION, &bearer);
    assert_eq!(api.send(page).await.0, StatusCode::OK);

    // A refused request does not reach its handler.
    let unsigned = json_request("PUT", "/api/v1/metrics/ci", r#"{"value": 1}"#);
    assert_eq!(api.send(unsigned).await.0, StatusCode::UNAUTHORIZED);
    let check = request("GET", "/api/v1/metrics/ci");
    let check = with_header(check, header::AUTHORIZATION, &bearer);
    assert_eq!(api.send(check).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refuses_web_pages_without_a_token() {
    let api = Api::new(config());
    for (name, value, expected) in [
        (
            header::ORIGIN,
            "https://evil.example",
            StatusCode::FORBIDDEN,
        ),
        (header::ORIGIN, "null", StatusCode::FORBIDDEN),
        // DNS rebinding: a page's own host name made to point at the daemon.
        (header::HOST, "evil.example:7920", StatusCode::FORBIDDEN),
        (header::ORIGIN, "http://localhost:5173", StatusCode::OK),
        (header::HOST, "127.0.0.1:7920", StatusCode::OK),
    ] {
        let health = with_header(request("GET", "/api/v1/health"), name.clone(), value);
        let (status, body) = api.send(health).await;
        assert_eq!(status, expected, "{name}: {value}: {body}");
    }

    // A refused request does not reach its handler.
    let put = json_request("PUT", "/api/v1/metrics/ci", r#"{"value": 1}"#);
    let evil = with_header(put, header::ORIGIN, "https://evil.example");
    let (status, body) = api.send(evil).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(error(&body).contains("web pages"), "{body}");
    let (status, _) = api.send(request("GET", "/api/v1/metrics/ci")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reports_system_figures() {
    let api = Api::new(config());
    let system: SystemView = api.get("/api/v1/system").await;
    assert!(system.cpu_count > 0);
    assert!(system.memory_total > 0);
    assert!((0.0..=100.0).contains(&system.cpu_percent));
}

#[tokio::test]
async fn pages_read_with_their_token() {
    for daemon_token in [None, Some(TOKEN.to_owned())] {
        let api = Api::new(Config {
            token: daemon_token.clone(),
            ..config()
        });
        let token = PageToken::issue();
        let bearer = format!("Bearer {}", token.as_str());
        let from_page = |request: Request<Body>, origin: &str| {
            let request = with_header(request, header::ORIGIN, origin);
            with_header(request, header::AUTHORIZATION, &bearer)
        };
        let put = json_request("PUT", "/api/v1/metrics/ci", r#"{"value": 1}"#);
        let put = match &daemon_token {
            Some(t) => with_header(put, header::AUTHORIZATION, &format!("Bearer {t}")),
            None => put,
        };
        assert!(api.send(put).await.0.is_success());

        // Reads, from any origin, with CORS headers for it.
        for (path, origin) in [
            ("/api/v1/metrics/ci", "https://example.com"),
            ("/api/v1/metrics", "null"),
            ("/api/v1/displays", "http://192.168.1.5:8000"),
            ("/api/v1/system", "null"),
        ] {
            let response = api
                .router
                .clone()
                .oneshot(from_page(request("GET", path), origin))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let allowed = response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN);
            assert_eq!(allowed.unwrap(), origin, "{path}");
        }

        // Nothing else, and the request does not reach its handler.
        for request in [
            json_request("PUT", "/api/v1/metrics/ci", r#"{"value": 2}"#),
            post(&format!("/api/v1/displays/{DISPLAY}/clear"), ""),
            request("GET", "/api/v1/web/chrome"),
        ] {
            let (status, body) = api.send(from_page(request, "null")).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert!(error(&body).contains("page token"), "{body}");
        }
        assert!(api.log.take().is_empty());

        // A token of a page no longer shown is just a wrong token.
        drop(token);
        let (status, _) = api
            .send(from_page(
                request("GET", "/api/v1/metrics"),
                "https://example.com",
            ))
            .await;
        let refused = if daemon_token.is_some() {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::FORBIDDEN
        };
        assert_eq!(status, refused);
    }
}

#[tokio::test]
async fn answers_preflights_for_pages() {
    let api = Api::new(Config {
        token: Some(TOKEN.into()),
        ..config()
    });
    let preflight = |path: &str| {
        let request = request("OPTIONS", path);
        let request = with_header(request, header::ORIGIN, "https://example.com");
        let request = with_header(request, header::ACCESS_CONTROL_REQUEST_METHOD, "GET");
        let request = with_header(
            request,
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization",
        );
        with_header(
            request,
            HeaderName::from_static("access-control-request-private-network"),
            "true",
        )
    };
    let response = api
        .router
        .clone()
        .oneshot(preflight("/api/v1/metrics/claude-code"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let headers = response.headers();
    assert_eq!(
        headers[header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "https://example.com"
    );
    assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_METHODS], "GET");
    assert_eq!(
        headers[header::ACCESS_CONTROL_ALLOW_HEADERS],
        "authorization"
    );
    assert_eq!(headers["access-control-allow-private-network"], "true");
    // Only for what pages may read.
    let (status, _) = api
        .send(preflight(&format!("/api/v1/displays/{DISPLAY}/clear")))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
