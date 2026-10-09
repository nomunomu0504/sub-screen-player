//! Starting the daemon at login: launchd (macOS), systemd user units (Linux) and the
//! `Run` registry key (Windows).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Arguments the autostart entry passes to `ssp`.
fn serve_args(config: &Path) -> Vec<String> {
    vec![
        "serve".into(),
        "--config".into(),
        config.display().to_string(),
    ]
}

/// The program the autostart entry runs. Homebrew installs `ssp` in a folder named after its
/// version (`<prefix>/Cellar/ssp/0.5.1/bin/ssp`), which goes away on upgrade, and Linux reports
/// that path even when `ssp` was run through the link in `<prefix>/bin`; its link that stays
/// (`<prefix>/opt/ssp/bin/ssp`) is used instead.
fn current_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("cannot locate the ssp executable")?;
    Ok(lasting_path(&exe)
        .filter(|path| path.exists())
        .unwrap_or(exe))
}

/// `<prefix>/Cellar/<name>/<version>/<rest>` → `<prefix>/opt/<name>/<rest>`.
fn lasting_path(exe: &Path) -> Option<PathBuf> {
    let parts: Vec<_> = exe.components().collect();
    let cellar = parts
        .iter()
        .rposition(|part| part.as_os_str() == "Cellar")?;
    if parts.len() < cellar + 4 {
        return None;
    }
    let mut path: PathBuf = parts[..cellar].iter().collect();
    path.push("opt");
    path.push(parts[cellar + 1]);
    path.extend(&parts[cellar + 3..]);
    Some(path)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("cannot run {:?}", command.get_program()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{:?} failed: {}{}",
            command.get_program(),
            stderr.trim(),
            stdout.trim()
        );
    }
    Ok(stdout)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const LABEL: &str = "dev.sub-screen-player.ssp";

    fn plist_path() -> Result<PathBuf> {
        let home = directories::BaseDirs::new().context("no home directory")?;
        Ok(home
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    fn log_path() -> Result<PathBuf> {
        let home = directories::BaseDirs::new().context("no home directory")?;
        Ok(home.home_dir().join("Library/Logs/sub-screen-player.log"))
    }

    fn domain() -> Result<String> {
        let uid = run(Command::new("id").arg("-u"))?;
        Ok(format!("gui/{}", uid.trim()))
    }

    fn xml_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    pub fn install(config: &Path) -> Result<String> {
        let exe = current_exe()?;
        let mut args = vec![exe.display().to_string()];
        args.extend(serve_args(config));
        let args: String = args
            .iter()
            .map(|a| format!("\n        <string>{}</string>", xml_escape(a)))
            .collect();
        let log = xml_escape(&log_path()?.display().to_string());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#
        );
        let path = plist_path()?;
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        std::fs::write(&path, plist).with_context(|| format!("cannot write {}", path.display()))?;
        let domain = domain()?;
        let service = format!("{domain}/{LABEL}");
        // Replace a running instance; failing because none was loaded is fine. `bootout`
        // returns before the old daemon has exited, and `bootstrap` fails until it has
        // ("Bootstrap failed: 5: Input/output error"), so wait for it and retry a little.
        let _ = run(Command::new("launchctl").args(["bootout", &service]));
        let started = std::time::Instant::now();
        while run(Command::new("launchctl").args(["print", &service])).is_ok()
            && started.elapsed() < std::time::Duration::from_secs(15)
        {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let mut attempts = 0;
        loop {
            match run(Command::new("launchctl")
                .args(["bootstrap", &domain])
                .arg(&path))
            {
                Ok(_) => break,
                Err(_) if attempts < 10 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                Err(err) => return Err(err),
            }
        }
        Ok(format!(
            "Installed {} and started the daemon.\nLogs: {}",
            path.display(),
            log_path()?.display()
        ))
    }

    pub fn uninstall() -> Result<String> {
        let _ = run(Command::new("launchctl").args(["bootout", &format!("{}/{LABEL}", domain()?)]));
        let path = plist_path()?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(format!(
                "Stopped the daemon and removed {}.",
                path.display()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Not installed.".into()),
            Err(e) => Err(e).with_context(|| format!("cannot remove {}", path.display())),
        }
    }

    pub fn status() -> Result<String> {
        let path = plist_path()?;
        if !path.exists() {
            return Ok("Not installed.".into());
        }
        let printed =
            run(Command::new("launchctl").args(["print", &format!("{}/{LABEL}", domain()?)]));
        let state = printed
            .ok()
            .and_then(|out| {
                out.lines()
                    .find_map(|l| l.trim().strip_prefix("state = ").map(str::to_string))
            })
            .unwrap_or_else(|| "not loaded".into());
        Ok(format!("Installed: {}\nState: {state}", path.display()))
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    const UNIT: &str = "sub-screen-player.service";

    fn unit_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().context("no home directory")?;
        Ok(dirs.config_dir().join("systemd/user").join(UNIT))
    }

    fn systemctl(args: &[&str]) -> Result<String> {
        run(Command::new("systemctl").arg("--user").args(args))
    }

    /// Quotes an argument for a unit file's `ExecStart=`.
    fn quote(arg: &str) -> String {
        format!(
            "\"{}\"",
            arg.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
        )
    }

    pub fn install(config: &Path) -> Result<String> {
        let exe = current_exe()?;
        let mut command = vec![quote(&exe.display().to_string())];
        command.extend(serve_args(config).iter().map(|a| quote(a)));
        let unit = format!(
            "[Unit]\n\
             Description=sub-screen-player daemon\n\
             \n\
             [Service]\n\
             ExecStart={}\n\
             Restart=on-failure\n\
             RestartSec=3\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            command.join(" ")
        );
        let path = unit_path()?;
        std::fs::create_dir_all(path.parent().expect("has a parent"))?;
        std::fs::write(&path, unit).with_context(|| format!("cannot write {}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT])?;
        Ok(format!(
            "Installed {} and started the daemon.\nLogs: journalctl --user -u {UNIT}\n\
             If no display is found, install the udev rule from contrib/linux (see README).",
            path.display()
        ))
    }

    pub fn uninstall() -> Result<String> {
        let path = unit_path()?;
        if !path.exists() {
            return Ok("Not installed.".into());
        }
        let _ = systemctl(&["disable", "--now", UNIT]);
        std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        Ok(format!(
            "Stopped the daemon and removed {}.",
            path.display()
        ))
    }

    pub fn status() -> Result<String> {
        let path = unit_path()?;
        if !path.exists() {
            return Ok("Not installed.".into());
        }
        let state = systemctl(&["is-active", UNIT]).unwrap_or_else(|_| "inactive".into());
        Ok(format!(
            "Installed: {}\nState: {}",
            path.display(),
            state.trim()
        ))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "sub-screen-player";

    fn log_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new().context("no home directory")?;
        Ok(dirs
            .data_local_dir()
            .join("sub-screen-player")
            .join("ssp.log"))
    }

    fn command_line(config: &Path) -> Result<Vec<String>> {
        let mut args = vec![current_exe()?.display().to_string()];
        args.extend(serve_args(config));
        args.extend([
            "--detach".into(),
            "--log-file".into(),
            log_path()?.display().to_string(),
        ]);
        Ok(args)
    }

    pub fn install(config: &Path) -> Result<String> {
        let args = command_line(config)?;
        let line: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            .context("cannot open the Run registry key")?;
        key.set_value(VALUE, &line.join(" "))
            .context("cannot write the Run registry value")?;
        // Start it now as well.
        Command::new(&args[0])
            .args(&args[1..])
            .spawn()
            .context("cannot start the daemon")?;
        Ok(format!(
            "Registered the daemon to start at login and started it.\nLogs: {}",
            log_path()?.display()
        ))
    }

    pub fn uninstall() -> Result<String> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE)
            .context("cannot open the Run registry key")?;
        match key.delete_value(VALUE) {
            Ok(()) => Ok(
                "Removed the login entry. A running daemon keeps running until you \
                          sign out (or end ssp.exe in Task Manager)."
                    .into(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Not installed.".into()),
            Err(e) => Err(e).context("cannot delete the Run registry value"),
        }
    }

    pub fn status() -> Result<String> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_READ)
            .context("cannot open the Run registry key")?;
        match key.get_value::<String, _>(VALUE) {
            Ok(line) => Ok(format!("Installed: {line}")),
            Err(_) => Ok("Not installed.".into()),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::*;

    pub fn install(_config: &Path) -> Result<String> {
        anyhow::bail!("autostart is not supported on this platform; run `ssp serve` yourself")
    }

    pub fn uninstall() -> Result<String> {
        install(Path::new(""))
    }

    pub fn status() -> Result<String> {
        install(Path::new(""))
    }
}

pub use platform::{install, status, uninstall};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homebrew_installs_run_from_the_link_that_stays() {
        assert_eq!(
            lasting_path(Path::new(
                "/home/linuxbrew/.linuxbrew/Cellar/ssp/0.5.1/bin/ssp"
            )),
            Some(PathBuf::from("/home/linuxbrew/.linuxbrew/opt/ssp/bin/ssp"))
        );
        assert_eq!(
            lasting_path(Path::new("/opt/homebrew/Cellar/ssp/0.6.0/bin/ssp")),
            Some(PathBuf::from("/opt/homebrew/opt/ssp/bin/ssp"))
        );
        assert_eq!(lasting_path(Path::new("/usr/local/bin/ssp")), None);
        assert_eq!(
            lasting_path(Path::new("/opt/homebrew/Cellar/ssp/0.6.0")),
            None
        );
    }
}
