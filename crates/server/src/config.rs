//! The daemon's TOML configuration. Every field has a default, so an empty file is valid.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ssp_core::{Fit, StopAction};

/// Port the daemon listens on unless configured otherwise.
pub const DEFAULT_PORT: u16 = 7920;

/// Shortest token accepted for the API.
const MIN_TOKEN_LEN: usize = 16;

/// A commented config file with the default values, written by `ssp config init`.
pub const TEMPLATE: &str = r##"# sub-screen-player configuration. Every setting is optional.

# Address of the HTTP/WebSocket API. Listening on anything but loopback
# (127.0.0.1 / ::1) requires `token`.
listen = "127.0.0.1:7920"
# token = "a long random string"

[display]
# brightness = 80        # percent, applied when a display connects
max_fps = 60
quality = 85             # JPEG quality, 1-100
min_quality = 70         # lowest quality used to keep up with fast animations and streams
                         # (the same as quality keeps it fixed)
partial_updates = true   # send only the changed parts of a frame (D92), e.g. a clock's seconds
on_exit = "leave"        # "leave", "save-last", "clear" or "sleep"

[startup]
show = "clock"           # "clock", "dashboard", "image", "web", "rotation" or "nothing"
# image = "/path/to/picture.png"   # a video works too (needs ffmpeg, see [video])
# url = "https://example.com/panel.html"   # for show = "web" (see [web])
# reload = 600           # reload the web page every this many seconds
fit = "contain"          # "contain", "cover" or "stretch"

# Change the screen by itself at set times (local time). Each entry sets what it names from
# its time on: `show` (with the options of [startup]; it also switches the screen on),
# `brightness` and `power`. `days` limits it to some days ("mon" .. "sun"), `display` to one
# display. Changes by hand last until the next entry.
# [[schedule]]
# at = "09:00"
# days = ["mon", "tue", "wed", "thu", "fri"]
# show = "dashboard"
# brightness = 100
#
# [[schedule]]
# at = "19:00"
# show = "clock"
# brightness = 40
#
# [[schedule]]
# at = "01:00"
# power = "off"

# Screens shown in turn, for show = "rotation" (in [startup] or a schedule entry).
# [rotation]
# every = 30             # seconds per screen, unless a screen says `seconds`
# show = ["clock", "dashboard", { show = "web", url = "file:///home/me/panel.html", seconds = 60 }]

[clock]
seconds = true
# time_format = "%H:%M"  # strftime; overrides `seconds`
date_format = "%Y-%m-%d %a"   # "" hides the date
# weekdays = ["日", "月", "火", "水", "木", "金", "土"]   # names for %a and %A, from Sunday
color = "#F0F2F8"
background = "#000000"

[dashboard]
# Panels from left to right: "clock", "cpu", "memory", "network", "disk", "claude-code"
# (tokens Claude Code used, see [claude_code]), and "metric:<id>" for figures sent with
# `ssp metric set <id>` or PUT /api/v1/metrics/<id>.
# The clock panel uses the formats of [clock].
widgets = ["clock", "cpu", "memory", "network", "disk"]
color = "#F0F2F8"        # text
accent = "#6EE7B7"       # graphs
background = "#000000"

[web]
# Web pages are drawn by headless Chrome, downloaded on first use with `ssp web --install`
# (about 100 MB, from Google's Chrome for Testing). Or use an installed Chrome, Chromium or Edge:
# chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
auto_download = false    # download without asking when a page is shown and none is there

[video]
# Videos are played by ffmpeg, found on PATH or where it is usually installed.
# ffmpeg = "/opt/homebrew/bin/ffmpeg"

[claude_code]
# Claude Code's directory, whose projects/ holds the session logs the "claude-code" panel
# reads (on this computer only). Default: $CLAUDE_CONFIG_DIR, else ~/.config/claude and ~/.claude.
# dir = "~/.claude"

[drivers]
# enable = ["d92"]       # use only these drivers (experimental ones included)
# disable = []           # never use these drivers
"##;

/// Top-level configuration (`config.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Address of the HTTP/WebSocket API. Anything but a loopback address requires `token`.
    pub listen: SocketAddr,
    /// Bearer token clients must send. Optional on loopback, required otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// How displays are driven.
    pub display: DisplayConfig,
    /// What a display shows when it is connected.
    pub startup: StartupConfig,
    /// Changes at set times (`[[schedule]]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schedule: Vec<ScheduleEntry>,
    /// Screens shown in turn for `show = "rotation"`.
    pub rotation: RotationConfig,
    /// Look of the built-in clock.
    pub clock: ClockConfig,
    /// Contents and look of the built-in dashboard.
    pub dashboard: DashboardConfig,
    /// How web pages are drawn.
    pub web: WebConfig,
    /// How videos are played.
    pub video: VideoConfig,
    /// Where the `claude-code` dashboard panel reads its figures.
    pub claude_code: ClaudeCodeConfig,
    /// Which device drivers the daemon uses.
    pub drivers: DriversConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT)),
            token: None,
            display: DisplayConfig::default(),
            startup: StartupConfig::default(),
            schedule: Vec::new(),
            rotation: RotationConfig::default(),
            clock: ClockConfig::default(),
            dashboard: DashboardConfig::default(),
            web: WebConfig::default(),
            video: VideoConfig::default(),
            claude_code: ClaudeCodeConfig::default(),
            drivers: DriversConfig::default(),
        }
    }
}

/// How web pages are drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebConfig {
    /// A Chrome, Chromium or Edge program to use instead of a downloaded headless Chrome.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chrome: Option<PathBuf>,
    /// Download headless Chrome when a page is shown and none is installed, without asking.
    pub auto_download: bool,
}

/// How videos are played.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoConfig {
    /// The ffmpeg program. Default: found on `PATH` or where it is usually installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ffmpeg: Option<std::path::PathBuf>,
}

/// Where the `claude-code` dashboard panel reads Claude Code's session logs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClaudeCodeConfig {
    /// Claude Code's directory (`~` allowed); the logs are in its `projects/`. Default:
    /// `$CLAUDE_CONFIG_DIR`, else `~/.config/claude` and `~/.claude`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<std::path::PathBuf>,
}

/// Which device drivers the daemon uses (driver ids such as `"d92"`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DriversConfig {
    /// If not empty, only these drivers are used, experimental ones included.
    pub enable: Vec<String>,
    /// Drivers that are never used.
    pub disable: Vec<String>,
}

impl DriversConfig {
    /// The selection for [`ssp_core::Registry::select`].
    pub fn selection(&self) -> ssp_core::DriverSelection {
        ssp_core::DriverSelection {
            only: self.enable.clone(),
            exclude: self.disable.clone(),
        }
    }
}

/// Settings applied to every display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// Backlight in percent, set when a display connects. Unset leaves it as it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    /// Upper bound for the frame rate.
    pub max_fps: u32,
    /// JPEG quality (1..=100).
    pub quality: u8,
    /// Lowest JPEG quality used while the display falls behind fast frames (1..=100). Values
    /// above `quality` act like `quality`, i.e. a fixed quality.
    pub min_quality: u8,
    /// Send only the changed parts of a frame, on displays that can show partial images.
    pub partial_updates: bool,
    /// What to do to the screens when the daemon exits.
    pub on_exit: OnExit,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            brightness: None,
            max_fps: 60,
            quality: 85,
            min_quality: 70,
            partial_updates: true,
            on_exit: OnExit::Leave,
        }
    }
}

/// What to do to the screens when the daemon exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnExit {
    /// Leave the screen alone.
    #[default]
    Leave,
    /// Store the last frame on the device so it stays visible.
    SaveLast,
    /// Blank the screen.
    Clear,
    /// Switch the screen off.
    Sleep,
}

impl From<OnExit> for StopAction {
    fn from(value: OnExit) -> Self {
        match value {
            OnExit::Leave => Self::Leave,
            OnExit::SaveLast => Self::SaveLast,
            OnExit::Clear => Self::Clear,
            OnExit::Sleep => Self::Sleep,
        }
    }
}

/// What a display shows when it is connected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StartupConfig {
    /// `"clock"`, `"dashboard"`, `"image"`, `"web"` or `"nothing"`.
    pub show: StartupShow,
    /// Image or video file for `show = "image"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
    /// Page for `show = "web"`: an `http`, `https` or `file` URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Reload the page every this many seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload: Option<u64>,
    /// How the image is fitted: `"contain"`, `"cover"` or `"stretch"`.
    pub fit: FitName,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            show: StartupShow::Clock,
            image: None,
            url: None,
            reload: None,
            fit: FitName::Contain,
        }
    }
}

/// See [`StartupConfig::show`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupShow {
    /// Leave whatever the device shows.
    Nothing,
    /// The built-in clock.
    #[default]
    Clock,
    /// The built-in dashboard.
    Dashboard,
    /// `startup.image`.
    Image,
    /// `startup.url`.
    Web,
    /// The screens of `[rotation]`, in turn.
    Rotation,
}

impl StartupConfig {
    /// What to show, as for a schedule entry or a turn of a rotation.
    pub fn spec(&self) -> ShowSpec<'_> {
        ShowSpec {
            show: self.show,
            image: self.image.as_deref(),
            url: self.url.as_deref(),
            reload: self.reload,
            fit: self.fit,
        }
    }
}

/// What to show: `[startup]`, a `[[schedule]]` entry or a turn of `[rotation]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShowSpec<'a> {
    /// The kind of screen.
    pub show: StartupShow,
    /// Image or video file for `image`.
    pub image: Option<&'a Path>,
    /// Page for `web`.
    pub url: Option<&'a str>,
    /// Reload the page every this many seconds.
    pub reload: Option<u64>,
    /// How an image or video is fitted.
    pub fit: FitName,
}

impl ShowSpec<'_> {
    /// Checks that the options the kind needs are there; `rotation` says whether `[rotation]`
    /// has screens.
    fn validate(&self, rotation: bool) -> Result<(), String> {
        match self.show {
            StartupShow::Image if self.image.is_none() => {
                Err("show = \"image\" needs image".into())
            }
            StartupShow::Web if self.url.is_none() => Err("show = \"web\" needs url".into()),
            StartupShow::Rotation if !rotation => {
                Err("show = \"rotation\" needs screens in [rotation] show".into())
            }
            _ if self.reload == Some(0) => Err("reload must be at least 1".into()),
            _ => Ok(()),
        }
    }
}

/// `power` of a schedule entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Power {
    /// Switch the screen on.
    On,
    /// Switch the screen off.
    Off,
}

/// One `[[schedule]]` entry: what changes at a time of day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEntry {
    /// Local time, `"HH:MM"`.
    pub at: String,
    /// Days it applies on, `"mon"` to `"sun"`; every day when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<String>,
    /// The id of the display it applies to; every display when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// What to show from then on (as `[startup] show`). Also switches the screen on, unless
    /// `power` says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<StartupShow>,
    /// Image or video file for `show = "image"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
    /// Page for `show = "web"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Reload the page every this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<u64>,
    /// How an image or video is fitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitName>,
    /// Backlight in percent from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    /// Screen on or off from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<Power>,
}

impl ScheduleEntry {
    /// What it shows, if it shows something.
    pub fn spec(&self) -> Option<ShowSpec<'_>> {
        Some(ShowSpec {
            show: self.show?,
            image: self.image.as_deref(),
            url: self.url.as_deref(),
            reload: self.reload,
            fit: self.fit.unwrap_or_default(),
        })
    }

    /// The time of day as hour and minute.
    pub fn time(&self) -> Result<(i8, i8), String> {
        let invalid = || format!("at = {:?} is not a time like \"07:30\"", self.at);
        let (hour, minute) = self.at.split_once(':').ok_or_else(invalid)?;
        let hour: i8 = hour.parse().map_err(|_| invalid())?;
        let minute: i8 = minute.parse().map_err(|_| invalid())?;
        if !(0..24).contains(&hour) || !(0..60).contains(&minute) || self.at.len() != 5 {
            return Err(invalid());
        }
        Ok((hour, minute))
    }

    /// The days it applies on, Monday = 0; all of them when `days` is empty.
    pub fn weekdays(&self) -> Result<[bool; 7], String> {
        if self.days.is_empty() {
            return Ok([true; 7]);
        }
        let mut on = [false; 7];
        for day in &self.days {
            let i = WEEKDAYS
                .iter()
                .position(|d| d.eq_ignore_ascii_case(day))
                .ok_or_else(|| format!("{day:?} is not a day: use {}", WEEKDAYS.join(", ")))?;
            on[i] = true;
        }
        Ok(on)
    }

    fn validate(&self, rotation: bool) -> Result<(), String> {
        self.time()?;
        self.weekdays()?;
        if self.show.is_none()
            && (self.image.is_some() || self.url.is_some() || self.reload.is_some())
        {
            return Err("image, url and reload need show".into());
        }
        if self.show.is_none() && self.brightness.is_none() && self.power.is_none() {
            return Err("says nothing to do: give show, brightness or power".into());
        }
        if self.brightness.is_some_and(|b| b > 100) {
            return Err("brightness must be 0..=100".into());
        }
        self.spec().map_or(Ok(()), |spec| spec.validate(rotation))
    }
}

/// Day names of schedule entries, from Monday.
pub const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Screens shown in turn (`[rotation]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RotationConfig {
    /// Seconds each screen is shown, unless it says otherwise.
    pub every: u64,
    /// The screens, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<RotationItem>,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            every: 30,
            show: Vec::new(),
        }
    }
}

impl RotationConfig {
    fn validate(&self) -> Result<(), String> {
        if self.every == 0 {
            return Err("rotation.every must be at least 1".into());
        }
        for item in &self.show {
            let (spec, seconds) = item.spec();
            if spec.show == StartupShow::Rotation {
                return Err("a rotation cannot show a rotation".into());
            }
            spec.validate(true).map_err(|e| format!("rotation: {e}"))?;
            if seconds == Some(0) {
                return Err("rotation: seconds must be at least 1".into());
            }
        }
        Ok(())
    }
}

/// A screen of `[rotation]`: a name such as `"clock"`, or a table with options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RotationItem {
    /// Just the kind, e.g. `"clock"`.
    Name(StartupShow),
    /// The kind with options, e.g. `{ show = "web", url = "...", seconds = 60 }`.
    Spec(RotationSpec),
}

impl RotationItem {
    /// What it shows, and its own number of seconds if it has one.
    pub fn spec(&self) -> (ShowSpec<'_>, Option<u64>) {
        match self {
            Self::Name(show) => (
                ShowSpec {
                    show: *show,
                    image: None,
                    url: None,
                    reload: None,
                    fit: FitName::default(),
                },
                None,
            ),
            Self::Spec(spec) => (
                ShowSpec {
                    show: spec.show,
                    image: spec.image.as_deref(),
                    url: spec.url.as_deref(),
                    reload: spec.reload,
                    fit: spec.fit.unwrap_or_default(),
                },
                spec.seconds,
            ),
        }
    }
}

/// See [`RotationItem::Spec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RotationSpec {
    /// The kind of screen.
    pub show: StartupShow,
    /// Image or video file for `show = "image"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
    /// Page for `show = "web"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Reload the page every this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<u64>,
    /// How an image or video is fitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitName>,
    /// Seconds this screen is shown, instead of `every`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// Serializable [`Fit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FitName {
    /// See [`Fit::Contain`].
    #[default]
    Contain,
    /// See [`Fit::Cover`].
    Cover,
    /// See [`Fit::Stretch`].
    Stretch,
}

impl From<FitName> for Fit {
    fn from(value: FitName) -> Self {
        match value {
            FitName::Contain => Self::Contain,
            FitName::Cover => Self::Cover,
            FitName::Stretch => Self::Stretch,
        }
    }
}

/// Look of the built-in clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    /// Show seconds. Ignored when `time_format` is set.
    pub seconds: bool,
    /// strftime-style format of the big line, e.g. `"%H:%M"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_format: Option<String>,
    /// strftime-style format of the small line. Empty hides it.
    pub date_format: String,
    /// Names used for `%a` and `%A`, from Sunday, e.g. `["日", "月", ...]`. Empty keeps English.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weekdays: Vec<String>,
    /// Text color, `#RRGGBB`.
    pub color: String,
    /// Background color, `#RRGGBB`.
    pub background: String,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            seconds: true,
            time_format: None,
            date_format: "%Y-%m-%d %a".into(),
            weekdays: Vec::new(),
            color: "#F0F2F8".into(),
            background: "#000000".into(),
        }
    }
}

impl ClockConfig {
    /// The format of the big line.
    pub fn time_format(&self) -> &str {
        match (&self.time_format, self.seconds) {
            (Some(format), _) => format,
            (None, true) => "%H:%M:%S",
            (None, false) => "%H:%M",
        }
    }
}

/// Contents and look of the built-in dashboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DashboardConfig {
    /// Panels from left to right.
    pub widgets: Vec<Widget>,
    /// Text color, `#RRGGBB`.
    pub color: String,
    /// Color of graphs and marks, `#RRGGBB`.
    pub accent: String,
    /// Background color, `#RRGGBB`.
    pub background: String,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            widgets: vec![
                Widget::Clock,
                Widget::Cpu,
                Widget::Memory,
                Widget::Network,
                Widget::Disk,
            ],
            color: "#F0F2F8".into(),
            accent: "#6EE7B7".into(),
            background: "#000000".into(),
        }
    }
}

/// A panel of the dashboard. Written as a string: `"clock"`, `"cpu"`, `"memory"`,
/// `"network"`, `"disk"`, `"claude-code"` or `"metric:<id>"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Widget {
    /// The time and date, formatted as in `[clock]`.
    Clock,
    /// CPU use with a graph of the last minute.
    Cpu,
    /// Memory use with a graph of the last minute.
    Memory,
    /// Download and upload speed with a graph of the last minute.
    Network,
    /// Space used on the system disk.
    Disk,
    /// Tokens Claude Code used in the current 5-hour block and today, from its local logs.
    ClaudeCode,
    /// A metric sent from outside (`PUT /api/v1/metrics/{id}`).
    Metric(String),
}

impl std::str::FromStr for Widget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "clock" => Self::Clock,
            "cpu" => Self::Cpu,
            "memory" => Self::Memory,
            "network" => Self::Network,
            "disk" => Self::Disk,
            "claude-code" => Self::ClaudeCode,
            _ => match s.strip_prefix("metric:") {
                Some(id) => {
                    crate::metrics::validate_id(id)?;
                    Self::Metric(id.to_owned())
                }
                None => {
                    return Err(format!(
                        "unknown widget {s:?} (expected clock, cpu, memory, network, disk, claude-code or metric:<id>)"
                    ));
                }
            },
        })
    }
}

impl std::fmt::Display for Widget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clock => f.write_str("clock"),
            Self::Cpu => f.write_str("cpu"),
            Self::Memory => f.write_str("memory"),
            Self::Network => f.write_str("network"),
            Self::Disk => f.write_str("disk"),
            Self::ClaudeCode => f.write_str("claude-code"),
            Self::Metric(id) => write!(f, "metric:{id}"),
        }
    }
}

impl TryFrom<String> for Widget {
    type Error = String;

    fn try_from(value: String) -> Result<Self, String> {
        value.parse()
    }
}

impl From<Widget> for String {
    fn from(value: Widget) -> Self {
        value.to_string()
    }
}

impl DashboardConfig {
    /// Checks the colors and the widget list.
    pub fn validate(&self) -> Result<(), String> {
        if self.widgets.is_empty() {
            return Err("dashboard.widgets needs at least one widget".into());
        }
        if self.widgets.len() > MAX_WIDGETS {
            return Err(format!(
                "dashboard.widgets takes at most {MAX_WIDGETS} widgets"
            ));
        }
        for color in [&self.color, &self.accent, &self.background] {
            if parse_color(color).is_none() {
                return Err(format!("{color:?} is not a #RRGGBB color"));
            }
        }
        Ok(())
    }
}

/// More panels than this would be too narrow to read on a bar display.
pub const MAX_WIDGETS: usize = 6;

/// Errors from loading or checking a configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists but could not be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The file is not valid TOML for this schema.
    #[error("invalid config {path}: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: Box<toml::de::Error>,
    },
    /// A value is out of range or inconsistent.
    #[error("invalid config: {0}")]
    Invalid(String),
}

impl Config {
    /// Reads `path`. A missing file yields the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.into(),
                    source,
                });
            }
        };
        let config: Self = toml::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.into(),
            source: Box::new(e),
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Checks values that the schema alone cannot.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |msg: String| Err(ConfigError::Invalid(msg));
        match &self.token {
            Some(token) if token.len() < MIN_TOKEN_LEN => {
                return invalid(format!("token must be at least {MIN_TOKEN_LEN} characters"));
            }
            None if !self.listen.ip().is_loopback() => {
                return invalid(format!(
                    "listening on {} exposes the API to the network; set `token` as well",
                    self.listen
                ));
            }
            _ => {}
        }
        if self.display.brightness.is_some_and(|b| b > 100) {
            return invalid("display.brightness must be 0..=100".into());
        }
        if !(1..=100).contains(&self.display.quality) {
            return invalid("display.quality must be 1..=100".into());
        }
        if !(1..=100).contains(&self.display.min_quality) {
            return invalid("display.min_quality must be 1..=100".into());
        }
        if self.display.max_fps == 0 {
            return invalid("display.max_fps must be at least 1".into());
        }
        let rotation = !self.rotation.show.is_empty();
        self.startup
            .spec()
            .validate(rotation)
            .map_err(|e| ConfigError::Invalid(format!("startup: {e}")))?;
        self.rotation.validate().map_err(ConfigError::Invalid)?;
        for (n, entry) in self.schedule.iter().enumerate() {
            entry.validate(rotation).map_err(|e| {
                ConfigError::Invalid(format!("schedule entry {} ({}): {e}", n + 1, entry.at))
            })?;
        }
        for color in [&self.clock.color, &self.clock.background] {
            if parse_color(color).is_none() {
                return invalid(format!("{color:?} is not a #RRGGBB color"));
            }
        }
        if !matches!(self.clock.weekdays.len(), 0 | 7) {
            return invalid("clock.weekdays needs 7 names, from Sunday".into());
        }
        self.dashboard.validate().map_err(ConfigError::Invalid)?;
        Ok(())
    }
}

/// Parses `#RRGGBB`.
pub fn parse_color(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_schedule_and_a_rotation() {
        // The commented example of the template, uncommented.
        let example: String = TEMPLATE
            .lines()
            .skip_while(|l| !l.starts_with("# [[schedule]]"))
            .take_while(|l| !l.starts_with("[clock]"))
            .filter(|l| l.starts_with("# ") && !l.contains("Screens shown in turn"))
            .map(|l| format!("{}\n", &l[2..]))
            .collect();
        let config: Config = toml::from_str(&example).unwrap();
        config.validate().unwrap();
        assert_eq!(config.schedule.len(), 3);
        assert_eq!(
            config.schedule[0].weekdays().unwrap(),
            [true, true, true, true, true, false, false]
        );
        assert_eq!(config.schedule[2].power, Some(Power::Off));
        assert_eq!(config.rotation.show.len(), 3);
        let (spec, seconds) = config.rotation.show[2].spec();
        assert_eq!(
            (spec.show, spec.url.is_some(), seconds),
            (StartupShow::Web, true, Some(60))
        );
    }

    #[test]
    fn refuses_bad_schedules() {
        for (toml, expected) in [
            (
                "[[schedule]]\nat = \"7:30\"\nshow = \"clock\"",
                "not a time",
            ),
            (
                "[[schedule]]\nat = \"24:00\"\nshow = \"clock\"",
                "not a time",
            ),
            (
                "[[schedule]]\nat = \"07:30\"\ndays = [\"monday\"]\nshow = \"clock\"",
                "not a day",
            ),
            ("[[schedule]]\nat = \"07:30\"", "nothing to do"),
            ("[[schedule]]\nat = \"07:30\"\nshow = \"web\"", "needs url"),
            (
                "[[schedule]]\nat = \"07:30\"\nurl = \"https://a.b\"",
                "need show",
            ),
            ("[[schedule]]\nat = \"07:30\"\nbrightness = 101", "0..=100"),
            (
                "[[schedule]]\nat = \"07:30\"\nshow = \"rotation\"",
                "[rotation]",
            ),
            (
                "[rotation]\nshow = [\"rotation\"]",
                "a rotation cannot show a rotation",
            ),
            ("[rotation]\nevery = 0\nshow = [\"clock\"]", "at least 1"),
            ("[startup]\nshow = \"rotation\"", "[rotation]"),
        ] {
            let config: Config = toml::from_str(toml).unwrap_or_else(|e| panic!("{toml}: {e}"));
            let err = config.validate().unwrap_err().to_string();
            assert!(err.contains(expected), "{toml}: {err}");
        }
    }

    #[test]
    fn empty_file_gives_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config, Config::default());
        config.validate().unwrap();
    }

    #[test]
    fn template_matches_defaults() {
        assert_eq!(
            toml::from_str::<Config>(TEMPLATE).unwrap(),
            Config::default()
        );
    }

    #[test]
    fn defaults_round_trip() {
        let text = toml::to_string(&Config::default()).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), Config::default());
    }

    #[test]
    fn parses_a_full_file() {
        let config: Config = toml::from_str(
            r##"
            listen = "0.0.0.0:7920"
            token = "0123456789abcdef"
            [display]
            brightness = 70
            on_exit = "save-last"
            [startup]
            show = "image"
            image = "/tmp/a.png"
            fit = "cover"
            [clock]
            seconds = false
            date_format = ""
            weekdays = ["日", "月", "火", "水", "木", "金", "土"]
            color = "#ff8800"
            [dashboard]
            widgets = ["cpu", "network"]
            accent = "#FF8800"
            [drivers]
            enable = ["d92", "dnext"]
            "##,
        )
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.display.on_exit, OnExit::SaveLast);
        assert_eq!(config.clock.time_format(), "%H:%M");
        assert_eq!(parse_color(&config.clock.color), Some([0xff, 0x88, 0x00]));
        assert_eq!(config.drivers.selection().only, ["d92", "dnext"]);
        assert_eq!(config.dashboard.widgets, [Widget::Cpu, Widget::Network]);
    }

    #[test]
    fn checks_the_dashboard() {
        assert!(toml::from_str::<Config>("[dashboard]\nwidgets = [\"gpu\"]").is_err());
        assert!(toml::from_str::<Config>("[dashboard]\nwidgets = [\"metric:Bad\"]").is_err());
        let metric: Config = toml::from_str("[dashboard]\nwidgets = [\"metric:ci\"]").unwrap();
        assert_eq!(metric.dashboard.widgets, [Widget::Metric("ci".into())]);
        let empty: Config = toml::from_str("[dashboard]\nwidgets = []").unwrap();
        assert!(empty.validate().is_err());
        let bad_color: Config = toml::from_str("[dashboard]\naccent = \"green\"").unwrap();
        assert!(bad_color.validate().is_err());
    }

    #[test]
    fn network_listen_requires_a_token() {
        let config = Config {
            listen: "0.0.0.0:7920".parse().unwrap(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        let config = Config {
            token: Some("short".into()),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(toml::from_str::<Config>("lisen = \"127.0.0.1:1\"").is_err());
    }
}
