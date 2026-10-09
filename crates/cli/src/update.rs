//! Tells when a newer `ssp` is out (issue #18). `ssp status` reads
//! <https://subscreen.dev/latest.json>, which the website writes when it is built, at most once a
//! day: what it found (or that it could not) is kept in the data folder. The request is a plain
//! `GET` with `User-Agent: ssp/<version>`; nothing else is sent. The daemon never asks.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const LATEST_URL: &str = "https://subscreen.dev/latest.json";
/// How long a check is good for.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long `ssp status` waits for the answer at most.
const TIMEOUT: Duration = Duration::from_secs(2);
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the last check found.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Checked {
    /// When, in seconds since 1970.
    at: u64,
    /// The latest version, as last known.
    latest: Option<String>,
}

#[derive(Deserialize)]
struct Latest {
    version: String,
}

/// Tells on standard error that a newer version is out, when checks are on (`enabled`, and no
/// `SSP_NO_UPDATE_CHECK`) and standard error is a terminal, so scripts see nothing.
pub fn tell(enabled: bool) {
    let off = std::env::var_os("SSP_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0");
    if !enabled || off || !std::io::stderr().is_terminal() {
        return;
    }
    if let Some(latest) = latest()
        && newer(&latest, VERSION)
    {
        eprintln!("\n{}", message(&latest));
    }
}

/// The notice for `latest`.
fn message(latest: &str) -> String {
    format!(
        "ssp {latest} is out (this is {VERSION}): https://subscreen.dev/download/\n\
         Update with the installer, `brew upgrade ssp` or `scoop update ssp`, then restart the \
         daemon."
    )
}

/// The latest version: from the last check if it is less than a day old, else asked now.
fn latest() -> Option<String> {
    let path = cache_path()?;
    let cached: Option<Checked> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let (checked, asked) = check(cached, now, fetch);
    if asked {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, serde_json::to_vec(&checked).unwrap_or_default());
    }
    checked.latest
}

/// What is known at `now`, asking with `fetch` unless `cached` is less than a day old, and
/// whether it asked. A failed check counts as a check too (so a computer without internet does
/// not wait every time) and keeps the version found before.
fn check(
    cached: Option<Checked>,
    now: u64,
    fetch: impl FnOnce() -> Option<String>,
) -> (Checked, bool) {
    if let Some(cached) = &cached
        && cached.at <= now
        && now - cached.at < EVERY.as_secs()
    {
        return (cached.clone(), false);
    }
    let latest = fetch().or_else(|| cached.and_then(|c| c.latest));
    (Checked { at: now, latest }, true)
}

fn fetch() -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(LATEST_URL)
        .header("User-Agent", format!("ssp/{VERSION}"))
        .call()
        .ok()?;
    let latest: Latest = response.body_mut().read_json().ok()?;
    Some(latest.version)
}

fn cache_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "sub-screen-player")?;
    Some(dirs.data_local_dir().join("update-check.json"))
}

/// Whether version `candidate` is newer than `current` (`1.2.3`, with or without `v`).
pub fn newer(candidate: &str, current: &str) -> bool {
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()?;
    let mut parts = core.split('.').map(str::parse::<u64>);
    let major = parts.next()?.ok()?;
    let minor = parts.next().unwrap_or(Ok(0)).ok()?;
    let patch = parts.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.7.0", "0.6.1"));
        assert!(newer("v0.6.10", "0.6.9"));
        assert!(newer("1.0", "0.99.99"));
        assert!(!newer("0.6.1", "0.6.1"));
        assert!(!newer("0.6.0", "0.6.1"));
        assert!(!newer("garbage", "0.6.1"));
        assert!(!newer("0.7.0", ""));
    }

    #[test]
    fn asks_at_most_once_a_day() {
        let day = EVERY.as_secs();
        let found = |v: &str| Checked {
            at: 1000,
            latest: Some(v.into()),
        };
        // Fresh: the cached answer, without asking.
        let (checked, asked) = check(Some(found("0.7.0")), 1000 + day - 1, || panic!("asked"));
        assert_eq!((checked, asked), (found("0.7.0"), false));
        // Old, or never asked: asks.
        let (checked, asked) = check(Some(found("0.7.0")), 1000 + day, || Some("0.8.0".into()));
        assert_eq!(checked.latest.as_deref(), Some("0.8.0"));
        assert!(asked);
        let (checked, _) = check(None, 5, || Some("0.7.0".into()));
        assert_eq!(
            checked,
            Checked {
                at: 5,
                latest: Some("0.7.0".into())
            }
        );
        // A failed check keeps what was found before, and counts as a check.
        let (checked, asked) = check(Some(found("0.7.0")), 1000 + day, || None);
        assert_eq!(
            checked,
            Checked {
                at: 1000 + day,
                latest: Some("0.7.0".into())
            }
        );
        assert!(asked);
        // A clock set back does not leave the cache fresh for ever.
        let (_, asked) = check(Some(found("0.7.0")), 10, || None);
        assert!(asked);
    }

    #[test]
    fn says_how_to_update() {
        let text = message("9.9.9");
        assert!(text.starts_with(&format!("ssp 9.9.9 is out (this is {VERSION})")));
        assert!(text.contains("brew upgrade ssp"));
    }
}
