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
on_exit = "leave"        # "leave", "save-last", "clear" or "sleep"

[startup]
show = "clock"           # "clock", "image" or "nothing"
# image = "/path/to/picture.png"
fit = "contain"          # "contain", "cover" or "stretch"

[clock]
seconds = true
# time_format = "%H:%M"  # strftime; overrides `seconds`
date_format = "%Y-%m-%d %a"   # "" hides the date
# weekdays = ["日", "月", "火", "水", "木", "金", "土"]   # names for %a and %A, from Sunday
color = "#F0F2F8"
background = "#000000"

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
    /// Look of the built-in clock.
    pub clock: ClockConfig,
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
            clock: ClockConfig::default(),
            drivers: DriversConfig::default(),
        }
    }
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
    /// What to do to the screens when the daemon exits.
    pub on_exit: OnExit,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            brightness: None,
            max_fps: 60,
            quality: 85,
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
    /// `"clock"`, `"image"` or `"nothing"`.
    pub show: StartupShow,
    /// Image file for `show = "image"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
    /// How the image is fitted: `"contain"`, `"cover"` or `"stretch"`.
    pub fit: FitName,
}

impl Default for StartupConfig {
    fn default() -> Self {
        Self {
            show: StartupShow::Clock,
            image: None,
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
    /// `startup.image`.
    Image,
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
        if self.display.max_fps == 0 {
            return invalid("display.max_fps must be at least 1".into());
        }
        if self.startup.show == StartupShow::Image && self.startup.image.is_none() {
            return invalid("startup.show = \"image\" needs startup.image".into());
        }
        for color in [&self.clock.color, &self.clock.background] {
            if parse_color(color).is_none() {
                return invalid(format!("{color:?} is not a #RRGGBB color"));
            }
        }
        if !matches!(self.clock.weekdays.len(), 0 | 7) {
            return invalid("clock.weekdays needs 7 names, from Sunday".into());
        }
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
