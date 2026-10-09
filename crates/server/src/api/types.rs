//! Request and response bodies of the HTTP API, shared with clients such as the `ssp` CLI.

use serde::{Deserialize, Serialize};

use crate::config::{FitName, Widget};
pub use crate::metrics::MetricUpdate;
pub use crate::notify::Style as NotifyStyle;

/// `GET /api/v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// Always `"ok"`.
    pub status: String,
    /// Version of the daemon.
    pub version: String,
    /// Ids of the drivers the daemon uses; it only touches displays these drivers handle.
    /// Missing from daemons before 0.1.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drivers: Option<Vec<String>>,
    /// When the daemon started serving, or last applied its config again (RFC 3339). Missing
    /// from daemons before 0.6.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    /// Why the last reload of the config did not take: the daemon could not start with the new
    /// config and went back to the one before. Missing when the last reload took.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload_error: Option<String>,
}

/// Answer to `POST /api/v1/reload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reloaded {
    /// Where the daemon listens with the new config, e.g. `127.0.0.1:7920`.
    pub listen: String,
}

/// One display in `GET /api/v1/displays`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayView {
    /// Stable id used in URLs.
    pub id: String,
    /// Driver id, e.g. `"d92"`.
    pub driver: String,
    /// Model name.
    pub model: String,
    /// USB serial number.
    pub serial: String,
    /// Firmware version, if known.
    pub firmware: Option<String>,
    /// Whether the display is plugged in.
    pub connected: bool,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// What it shows: `"nothing"`, `"image"`, `"animation"`, `"clock"`, `"dashboard"` or
    /// `"stream"`.
    pub content: String,
    /// Supported operations.
    pub capabilities: CapabilitiesView,
    /// Counters of the current connection.
    pub stats: Option<StatsView>,
    /// The notification being shown (`POST /displays/{id}/notify`). Missing when there is none,
    /// and from daemons before 0.5.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification: Option<NotificationView>,
}

/// A notification being shown, in [`DisplayView`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NotificationView {
    /// The title.
    pub text: String,
    /// The line under the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `banner` or `full`.
    pub style: NotifyStyle,
    /// Background color, `#rrggbb`.
    pub color: String,
    /// Seconds until it ends; missing for a sticky one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds_left: Option<u64>,
    /// Whether it stays until dismissed or replaced.
    pub sticky: bool,
}

/// `POST /api/v1/displays/{id}/notify`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyRequest {
    /// The title (at most 80 characters).
    pub text: String,
    /// A smaller line under the title (at most 120 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// How long it stays, 1 to 86400 (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
    /// Stay until dismissed or replaced, instead of `seconds`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sticky: bool,
    /// `banner` (default) or `full`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<NotifyStyle>,
    /// `red`, `orange`, `yellow`, `green`, `blue` (default), `gray` or `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Switch a screen that is off on while it is shown, and off again after.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wake: bool,
}

/// Supported operations of a display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesView {
    /// Can show frames without storing them.
    pub live_frames: bool,
    /// Can store an image across power cycles.
    pub saved_frames: bool,
    /// Has adjustable backlight.
    pub brightness: bool,
    /// Can be switched off and on.
    pub power: bool,
    /// Can be blanked.
    pub clear: bool,
    /// Highest frame rate.
    pub max_fps: u32,
}

/// Counters of one display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsView {
    /// Frames received.
    pub submitted: u64,
    /// Frames sent to the device.
    pub shown: u64,
    /// Of the frames sent, those sent as their changed parts only (`[display] partial_updates`).
    /// Missing from older daemons.
    #[serde(default)]
    pub partial: u64,
    /// Frames replaced by newer ones before being sent.
    pub dropped: u64,
    /// Frames skipped because nothing changed.
    pub duplicates: u64,
    /// Encoding time of the last frame, in milliseconds.
    pub last_encode_ms: f64,
    /// Sending time of the last frame, in milliseconds.
    pub last_send_ms: f64,
    /// Size of the last frame sent, in bytes.
    pub last_bytes: usize,
    /// JPEG quality frames are encoded with now; lower than `[display] quality` while the
    /// display falls behind fast frames. Missing from daemons before 0.3.
    #[serde(default)]
    pub quality: u8,
}

/// `POST /api/v1/displays/{id}/brightness`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrightnessRequest {
    /// Backlight in percent (0..=100).
    pub percent: u8,
}

/// `POST /api/v1/displays/{id}/power`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerRequest {
    /// `true` switches the screen on, `false` off.
    pub on: bool,
}

/// `POST /api/v1/displays/{id}/clock`. Unset fields use the daemon's `[clock]` config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockRequest {
    /// Show seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds: Option<bool>,
    /// strftime-style format of the big line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_format: Option<String>,
    /// strftime-style format of the small line; empty hides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
    /// Names for `%a` and `%A`, from Sunday (7 names); empty keeps English.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekdays: Option<Vec<String>>,
    /// Text color, `#RRGGBB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Background color, `#RRGGBB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

/// `POST /api/v1/displays/{id}/web`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebRequest {
    /// An `http`, `https` or `file` URL.
    pub url: String,
    /// Reload the page every this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<u64>,
}

pub use crate::web::ChromeView;

/// `POST /api/v1/displays/{id}/dashboard`. Unset fields use the daemon's `[dashboard]` config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardRequest {
    /// Panels from left to right.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widgets: Option<Vec<Widget>>,
    /// Text color, `#RRGGBB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Color of graphs and marks, `#RRGGBB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// Background color, `#RRGGBB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
}

/// Query of `POST /api/v1/displays/{id}/image`. The body is a PNG, JPEG, GIF or WebP file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageQuery {
    /// How the image is fitted to the panel.
    pub fit: Option<FitName>,
    /// Also store the image on the device so it survives power cycles.
    pub persist: bool,
}

/// Query of `GET /api/v1/displays/{id}/stream` (WebSocket).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StreamQuery {
    /// What each binary message contains.
    pub format: StreamFormat,
    /// How encoded images are fitted to the panel.
    pub fit: Option<FitName>,
}

/// Contents of the binary messages of a stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamFormat {
    /// An image file (PNG, JPEG, GIF or WebP) of any size.
    #[default]
    Image,
    /// Raw 8-bit RGB pixels at the panel size.
    Rgb,
    /// Raw 8-bit RGBA pixels at the panel size; alpha is blended onto black.
    Rgba,
}

/// Body of every error response, and of error messages on a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// What went wrong.
    pub error: String,
}

/// A metric in `GET /api/v1/metrics` and `GET /api/v1/metrics/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricView {
    /// The id used in the URL and in `metric:<id>` dashboard widgets.
    pub id: String,
    /// Name shown above the value; the id when not set.
    pub label: String,
    /// The number shown, unless `text` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Text shown instead of a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Shown small after the value.
    pub unit: String,
    /// The line under the value.
    pub detail: String,
    /// Top of the graph; the largest recent value when not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Seconds until the value counts as stale.
    pub ttl: u64,
    /// When the value last changed (RFC 3339).
    pub updated: String,
    /// Seconds since the value last changed.
    pub age: u64,
    /// Whether the value is older than `ttl`.
    pub stale: bool,
    /// Recent values, oldest first.
    pub history: Vec<f32>,
}

/// `GET /api/v1/system`: the figures the dashboard draws. CPU use and network rates are
/// averaged since the previous request (requests less than 250 ms apart get the same figures).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemView {
    /// CPU use over all cores, 0-100.
    pub cpu_percent: f32,
    /// Number of logical CPUs.
    pub cpu_count: usize,
    /// One-minute load average; `null` on Windows.
    pub load: Option<f64>,
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Installed memory, bytes.
    pub memory_total: u64,
    /// Received bytes per second over all interfaces except loopback.
    pub rx_per_sec: f64,
    /// Sent bytes per second over all interfaces except loopback.
    pub tx_per_sec: f64,
    /// Used bytes of the system disk, if found.
    pub disk_used: Option<u64>,
    /// Size of the system disk in bytes, if found.
    pub disk_total: Option<u64>,
}

/// `GET /api/v1/schedule`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleView {
    /// Number of `[[schedule]]` entries; 0 without a schedule.
    pub entries: usize,
    /// Whether applying is paused (`POST /schedule/pause`).
    pub paused: bool,
    /// The latest entries that happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<ScheduleEventView>,
    /// The next entries to happen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<ScheduleEventView>,
}

/// Entries happening at one time, in [`ScheduleView`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEventView {
    /// Local time with its offset, e.g. `"2026-10-09T19:00:00+09:00"`.
    pub at: String,
    /// Positions of the entries in the config, from 1.
    pub entries: Vec<usize>,
    /// What each does, e.g. `"show clock, brightness 40"`.
    pub does: Vec<String>,
}
