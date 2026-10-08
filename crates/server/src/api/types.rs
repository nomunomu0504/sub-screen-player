//! Request and response bodies of the HTTP API, shared with clients such as the `ssp` CLI.

use serde::{Deserialize, Serialize};

use crate::config::{FitName, Widget};

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
#[serde(default)]
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

/// `POST /api/v1/displays/{id}/dashboard`. Unset fields use the daemon's `[dashboard]` config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
