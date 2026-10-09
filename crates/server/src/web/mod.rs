//! Web screens: pages rendered by headless Chrome.
//!
//! The browser is "chrome-headless-shell" from Google's Chrome for Testing, downloaded on first
//! use into the daemon's data folder (about 100 MB), or any Chrome, Chromium or Edge set as
//! `[web] chrome`. [`cdp`] drives it over the DevTools protocol.

pub mod cdp;
pub mod page_token;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::WebConfig;

/// Where Chrome for Testing lists its current versions.
const VERSIONS_URL: &str = "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
/// Rough size of the download, for messages.
pub const DOWNLOAD_SIZE: &str = "about 100 MB";

/// Only one download at a time.
static INSTALLING: Mutex<()> = Mutex::new(());

/// The browser settings, and where downloaded browsers are kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Web {
    /// `[web] chrome`, if set.
    configured: Option<PathBuf>,
    /// Download when needed without being asked (`[web] auto_download`).
    auto_download: bool,
    /// Folder of downloaded browsers.
    dir: PathBuf,
    /// Base URL of the API, given to shown pages as `window.ssp.api`.
    api: Option<String>,
}

/// The browser in use (`GET /api/v1/web/chrome`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChromeView {
    /// Whether a browser is ready.
    pub installed: bool,
    /// Its program.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The version, for a downloaded browser.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether it comes from `[web] chrome`.
    pub configured: bool,
    /// Where downloads go.
    pub dir: String,
}

impl Web {
    /// The settings of `[web]`, with downloads in the platform's data folder.
    pub fn new(config: &WebConfig) -> Self {
        let dir = directories::ProjectDirs::from("", "", "sub-screen-player").map_or_else(
            || std::env::temp_dir().join("sub-screen-player"),
            |dirs| dirs.data_local_dir().to_owned(),
        );
        Self {
            configured: config.chrome.clone(),
            auto_download: config.auto_download,
            dir: dir.join("chrome"),
            api: None,
        }
    }

    /// Tells shown pages where the API is (`window.ssp.api`): at `listen`, or on loopback when
    /// the daemon listens on all addresses.
    pub fn with_api(mut self, listen: SocketAddr) -> Self {
        let ip = match listen.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        self.api = Some(format!(
            "http://{}/api/v1",
            SocketAddr::new(ip, listen.port())
        ));
        self
    }

    /// Base URL of the API for shown pages, if known.
    pub fn api(&self) -> Option<&str> {
        self.api.as_deref()
    }

    /// Settings for tests: downloads go to `dir`.
    pub fn with_dir(configured: Option<PathBuf>, dir: PathBuf) -> Self {
        Self {
            configured,
            auto_download: false,
            dir,
            api: None,
        }
    }

    /// The browser to use, if there is one: `[web] chrome`, else the newest downloaded one.
    pub fn chrome(&self) -> Option<(PathBuf, Option<String>)> {
        if let Some(path) = &self.configured {
            return path.is_file().then(|| (path.clone(), None));
        }
        self.downloaded()
    }

    /// The browser to use, downloading one first if `[web] auto_download` allows it.
    pub fn ensure(&self) -> Result<PathBuf, String> {
        if let Some((path, _)) = self.chrome() {
            return Ok(path);
        }
        if let Some(path) = &self.configured {
            return Err(format!("[web] chrome: {} does not exist", path.display()));
        }
        if self.auto_download {
            return self.install().map(|(path, _)| path);
        }
        Err(format!(
            "web pages need headless Chrome, which is not installed yet: run `ssp web --install` \
             (downloads {DOWNLOAD_SIZE}) or set `[web] chrome` to an installed Chrome"
        ))
    }

    /// What `GET /api/v1/web/chrome` reports.
    pub fn status(&self) -> ChromeView {
        let found = self.chrome();
        ChromeView {
            installed: found.is_some(),
            path: found.as_ref().map(|(p, _)| p.display().to_string()),
            version: found.and_then(|(_, v)| v),
            configured: self.configured.is_some(),
            dir: self.dir.display().to_string(),
        }
    }

    /// Downloads the current chrome-headless-shell for this platform from Chrome for Testing
    /// and removes older ones. Returns the program and its version.
    pub fn install(&self) -> Result<(PathBuf, String), String> {
        let _one_at_a_time = INSTALLING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let platform = platform().ok_or_else(|| {
            "Chrome for Testing has no build for this platform: set `[web] chrome` to an \
             installed Chrome or Chromium"
                .to_owned()
        })?;
        let versions = String::from_utf8(download(VERSIONS_URL, None)?)
            .map_err(|_| "the list of Chrome versions is not text".to_owned())?;
        let (version, url) = pick(&versions, platform)?;
        if let Some((path, Some(have))) = self.downloaded()
            && have == version
        {
            return Ok((path, version));
        }

        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("cannot create {}: {e}", self.dir.display()))?;
        let zip_path = self.dir.join(format!("{version}.zip"));
        tracing::info!(%url, "downloading headless Chrome {version}");
        download(&url, Some(&zip_path))?;
        let partial = self.dir.join(format!("{version}.partial"));
        let unpacked = unpack(&zip_path, &partial);
        let _ = std::fs::remove_file(&zip_path);
        unpacked?;
        let target = self.dir.join(&version);
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&partial, &target)
            .map_err(|e| format!("cannot move Chrome into {}: {e}", target.display()))?;
        // Older downloads are no longer used.
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if entry.file_name() != version.as_str() && entry.path().is_dir() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let program = program_in(&target, platform);
        if !program.is_file() {
            return Err(format!("the download has no {}", program.display()));
        }
        tracing::info!(path = %program.display(), "headless Chrome {version} installed");
        Ok((program, version))
    }

    /// The newest downloaded browser and its version.
    fn downloaded(&self) -> Option<(PathBuf, Option<String>)> {
        let platform = platform()?;
        let mut versions: Vec<(Vec<u32>, String)> = std::fs::read_dir(&self.dir)
            .ok()?
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| Some((version_key(&name)?, name)))
            .collect();
        versions.sort();
        versions.into_iter().rev().find_map(|(_, version)| {
            let program = program_in(&self.dir.join(&version), platform);
            program.is_file().then_some((program, Some(version)))
        })
    }
}

/// `"155.0.8059.39"` as numbers, so versions sort correctly.
fn version_key(name: &str) -> Option<Vec<u32>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

/// The Chrome for Testing platform of this computer.
fn platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", "x86_64") => "mac-x64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linux-arm64",
        // Windows on ARM runs the x64 build.
        ("windows", "x86_64" | "aarch64") => "win64",
        ("windows", "x86") => "win32",
        _ => return None,
    })
}

fn program_in(version_dir: &Path, platform: &str) -> PathBuf {
    let name = if platform.starts_with("win") {
        "chrome-headless-shell.exe"
    } else {
        "chrome-headless-shell"
    };
    version_dir
        .join(format!("chrome-headless-shell-{platform}"))
        .join(name)
}

#[derive(Deserialize)]
struct Versions {
    channels: Channels,
}

#[derive(Deserialize)]
struct Channels {
    #[serde(rename = "Stable")]
    stable: Channel,
}

#[derive(Deserialize)]
struct Channel {
    version: String,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    #[serde(rename = "chrome-headless-shell", default)]
    headless_shell: Vec<Download>,
}

#[derive(Deserialize)]
struct Download {
    platform: String,
    url: String,
}

/// The stable version and its download for `platform`, from the Chrome for Testing list.
fn pick(versions: &str, platform: &str) -> Result<(String, String), String> {
    let versions: Versions = serde_json::from_str(versions)
        .map_err(|e| format!("cannot read the list of Chrome versions: {e}"))?;
    let stable = versions.channels.stable;
    let download = stable
        .downloads
        .headless_shell
        .into_iter()
        .find(|d| d.platform == platform)
        .ok_or_else(|| format!("Chrome for Testing has no headless Chrome for {platform}"))?;
    if !download
        .url
        .starts_with("https://storage.googleapis.com/chrome-for-testing-public/")
    {
        return Err(format!("unexpected download address {}", download.url));
    }
    if version_key(&stable.version).is_none() {
        return Err(format!("unexpected Chrome version {:?}", stable.version));
    }
    Ok((stable.version, download.url))
}

/// Downloads `url` with curl, into `to` or into memory. curl comes with macOS, Windows 10 and
/// later, and most Linux systems, and keeps a TLS stack out of `ssp`.
fn download(url: &str, to: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut command = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    command
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--proto", "=https", "--retry", "2", "--max-time", "1800"]);
    if let Some(path) = to {
        command.arg("--output").arg(path);
    }
    let output = command
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run curl to download headless Chrome: {e}"))?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!("cannot download {url}: {reason}"));
    }
    Ok(output.stdout)
}

/// Unpacks the zip at `zip_path` into `into`, keeping Unix permissions (the programs must stay
/// executable). Every entry's checksum is verified while reading.
fn unpack(zip_path: &Path, into: &Path) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(into);
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("the Chrome download is damaged: {e}"))?;
    archive
        .extract(into)
        .map_err(|e| format!("cannot unpack Chrome: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"timestamp":"2026-10-07T21:20:20.783Z","channels":{"Stable":{"channel":"Stable","version":"155.0.8059.39","revision":"1","downloads":{"chrome":[],"chrome-headless-shell":[{"platform":"linux64","url":"https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.39/linux64/chrome-headless-shell-linux64.zip"},{"platform":"mac-arm64","url":"https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.39/mac-arm64/chrome-headless-shell-mac-arm64.zip"}]}},"Beta":{}}}"#;

    #[test]
    fn tells_pages_where_the_api_is() {
        let web =
            |listen: &str| Web::with_dir(None, PathBuf::new()).with_api(listen.parse().unwrap());
        assert_eq!(
            web("127.0.0.1:7920").api(),
            Some("http://127.0.0.1:7920/api/v1")
        );
        assert_eq!(
            web("0.0.0.0:7920").api(),
            Some("http://127.0.0.1:7920/api/v1")
        );
        assert_eq!(web("[::]:7000").api(), Some("http://[::1]:7000/api/v1"));
        assert_eq!(
            web("192.168.1.5:7920").api(),
            Some("http://192.168.1.5:7920/api/v1")
        );
        assert_eq!(Web::with_dir(None, PathBuf::new()).api(), None);
    }

    #[test]
    fn picks_the_stable_download() {
        let (version, url) = pick(LIST, "mac-arm64").unwrap();
        assert_eq!(version, "155.0.8059.39");
        assert!(url.ends_with("/mac-arm64/chrome-headless-shell-mac-arm64.zip"));
        assert!(pick(LIST, "win64").unwrap_err().contains("win64"));
        let elsewhere = LIST.replace("https://storage.googleapis.com", "https://example.com");
        assert!(pick(&elsewhere, "linux64").is_err());
    }

    #[test]
    fn finds_the_newest_download() {
        let Some(platform) = platform() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("ssp-chrome-{}", std::process::id()));
        for version in ["99.0.1.2", "155.0.8059.39", "120.0.0.0", "not-a-version"] {
            let program = program_in(&dir.join(version), platform);
            std::fs::create_dir_all(program.parent().unwrap()).unwrap();
            std::fs::write(&program, b"").unwrap();
        }
        let web = Web::with_dir(None, dir.clone());
        let (program, version) = web.chrome().unwrap();
        assert_eq!(version.as_deref(), Some("155.0.8059.39"));
        assert!(program.starts_with(dir.join("155.0.8059.39")));
        assert!(web.status().installed);

        // A configured browser wins, and a missing one is reported.
        let missing = Web::with_dir(Some(dir.join("nothing")), dir.clone());
        assert!(missing.chrome().is_none());
        assert!(missing.ensure().unwrap_err().contains("does not exist"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn asks_for_an_install_when_nothing_is_there() {
        let web = Web::with_dir(None, std::env::temp_dir().join("ssp-no-chrome-here"));
        assert!(!web.status().installed);
        assert!(web.ensure().unwrap_err().contains("ssp web --install"));
    }
}
