//! `ssp update` (issue #23): replaces this `ssp` with another release, the way it was
//! installed. The caller then restarts the daemon of the autostart entry.
//!
//! - Installed with the installer (or by hand): the official installer runs again, into the
//!   folder of this `ssp`, for the release asked. It checks the SHA-256 and, on macOS and
//!   Linux, replaces the file by renaming it, which a running program allows. On Windows a
//!   running `ssp.exe` cannot be overwritten but can be renamed, so it is moved aside first
//!   and put back if anything goes wrong.
//! - Homebrew: `brew upgrade`.
//! - Scoop: Scoop does not replace an `ssp.exe` that runs, and `ssp update` is one, so the
//!   commands to run are shown instead.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::update;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SITE: &str = "https://subscreen.dev";

/// How `ssp` was installed.
#[derive(Debug, PartialEq, Eq)]
pub enum Method {
    /// With the installer or by hand, into this folder.
    Installer(PathBuf),
    Homebrew,
    Scoop,
}

/// How the `ssp` whose real path (links resolved) is `exe` was installed.
pub fn method(exe: &Path) -> Method {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let follows = |a: &str, b: &str| parts.windows(2).any(|w| w[0] == a && w[1] == b);
    if follows("cellar", "ssp") {
        Method::Homebrew
    } else if follows("apps", "ssp") && parts.iter().any(|p| p == "scoop") {
        Method::Scoop
    } else {
        Method::Installer(exe.parent().map(Path::to_path_buf).unwrap_or_default())
    }
}

/// `v1.2.3` for `1.2.3` or `v1.2.3`.
pub fn tag(version: &str) -> Result<String> {
    let version = version.trim().trim_start_matches('v');
    match update::parse(version) {
        Some(_) if version.split('.').count() == 3 => Ok(format!("v{version}")),
        _ => bail!("{version:?} is not a version like 0.7.1"),
    }
}

/// Updates to the latest release, or to `to`. With `check`, only tells. Returns the version
/// installed, if it installed one.
pub fn update(check: bool, to: Option<&str>) -> Result<Option<String>> {
    let target = match to {
        Some(version) => tag(version)?,
        None => tag(&update::latest_now().context(
            "cannot tell the latest version: https://subscreen.dev/latest.json did not answer",
        )?)?,
    };
    let version = target.trim_start_matches('v').to_owned();
    if version == VERSION {
        say!("ssp {VERSION} is installed already.");
        return Ok(None);
    }
    if to.is_none() && !update::newer(&version, VERSION) {
        say!("ssp {VERSION} is the latest.");
        return Ok(None);
    }
    if check {
        say!("ssp {version} is out (this is {VERSION}); `ssp update` installs it.");
        return Ok(None);
    }

    let exe = std::env::current_exe().context("cannot locate this ssp")?;
    let real = std::fs::canonicalize(&exe).unwrap_or_else(|_| exe.clone());
    match method(&real) {
        Method::Installer(dir) => {
            say!("Installing ssp {version} into {} …", dir.display());
            with_installer(&dir, &target, &version)?;
        }
        Method::Homebrew => {
            if to.is_some() {
                bail!(
                    "ssp was installed with Homebrew, which installs its newest version only; \
                     to install a given one, use the installer (see https://subscreen.dev/download/)"
                );
            }
            say!("Updating with Homebrew …");
            // The tap is read again only now and then; make it current first.
            run(Command::new("brew").args(["update", "--quiet"]))?;
            run(Command::new("brew").args(["upgrade", "nomunomu0504/tap/ssp"]))?;
            let installed = version_of(&exe)?;
            if installed != version {
                bail!(
                    "Homebrew has ssp {installed}, not {version} yet (it follows a release within \
                     the hour); try again later"
                );
            }
        }
        Method::Scoop => {
            say!(
                "ssp was installed with Scoop, which cannot replace ssp.exe while it runs (this \
                 command is one). Run these in PowerShell instead:\n\
                 \x20 taskkill /im ssp.exe /f\n\
                 \x20 scoop update ssp\n\
                 \x20 ssp service install"
            );
            return Ok(None);
        }
    }
    if update::newer(&version, VERSION) {
        say!("Updated ssp {VERSION} to {version}.");
    } else {
        say!("Installed ssp {version} (it was {VERSION}).");
    }
    Ok(Some(version))
}

/// Runs the installer into `dir` for release `tag`, then checks that `dir`'s `ssp` is
/// `version`. On Windows, the running `ssp.exe` is moved aside first and put back on failure.
fn with_installer(dir: &Path, tag: &str, version: &str) -> Result<()> {
    let name = if cfg!(windows) { "ssp.exe" } else { "ssp" };
    let exe = dir.join(name);
    let aside = dir.join(format!("{name}.old"));
    if cfg!(windows) {
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(&exe, &aside)
            .with_context(|| format!("cannot move {} aside", exe.display()))?;
    }
    let result = run_installer(dir, tag).and_then(|()| {
        let installed = version_of(&exe)?;
        if installed == version {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "the installer left ssp {installed}, not {version}"
            ))
        }
    });
    if result.is_err() && cfg!(windows) {
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::rename(&aside, &exe);
    }
    result
}

/// Downloads the installer for this system and runs it for release `tag` into `dir`.
fn run_installer(dir: &Path, tag: &str) -> Result<()> {
    let temp = std::env::temp_dir().join(format!("ssp-update-{}", std::process::id()));
    std::fs::create_dir_all(&temp).context("cannot create a temporary folder")?;
    let result = (|| {
        let script = if cfg!(windows) {
            "install.ps1"
        } else {
            "install.sh"
        };
        let file = temp.join(script);
        download(&format!("{SITE}/{script}"), &file)?;
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            command
        } else {
            Command::new("sh")
        };
        let status = command
            .arg(&file)
            .env("SSP_INSTALL_DIR", dir)
            .env("SSP_VERSION", tag)
            .env("SSP_FROM_UPDATE", "1")
            .stdin(Stdio::null())
            .status()
            .context("cannot run the installer")?;
        if !status.success() {
            bail!("the installer failed");
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&temp);
    result
}

/// Downloads `url` into `to` with curl (ssp itself has no TLS).
fn download(url: &str, to: &Path) -> Result<()> {
    let status = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" })
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--proto", "=https", "--max-time", "60", "--output"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .status()
        .context("cannot run curl")?;
    if !status.success() {
        bail!("cannot download {url}");
    }
    Ok(())
}

/// The version `ssp --version` of `exe` prints.
fn version_of(exe: &Path) -> Result<String> {
    let output = Command::new(exe)
        .arg("--version")
        .output()
        .with_context(|| format!("cannot run {}", exe.display()))?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .nth(1)
        .map(str::to_owned)
        .with_context(|| format!("{} did not tell its version", exe.display()))
}

fn run(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("cannot run {:?}", command.get_program()))?;
    if !status.success() {
        bail!("{:?} failed", command.get_program());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_how_ssp_was_installed() {
        assert_eq!(
            method(Path::new("/opt/homebrew/Cellar/ssp/0.7.1/bin/ssp")),
            Method::Homebrew
        );
        assert_eq!(
            method(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/ssp/0.7.1/bin/ssp"
            )),
            Method::Homebrew
        );
        assert_eq!(
            method(Path::new("/Users/me/scoop/apps/ssp/0.7.1/ssp.exe")),
            Method::Scoop
        );
        assert_eq!(
            method(Path::new("/home/me/.local/bin/ssp")),
            Method::Installer(PathBuf::from("/home/me/.local/bin"))
        );
        // An "ssp" folder alone is not Homebrew or Scoop.
        assert_eq!(
            method(Path::new("/opt/ssp/bin/ssp")),
            Method::Installer(PathBuf::from("/opt/ssp/bin"))
        );
    }

    #[test]
    fn takes_versions_with_or_without_v() {
        assert_eq!(tag("0.7.1").unwrap(), "v0.7.1");
        assert_eq!(tag("v0.8.0").unwrap(), "v0.8.0");
        assert!(tag("latest").is_err());
        assert!(tag("0.7").is_err());
    }
}
