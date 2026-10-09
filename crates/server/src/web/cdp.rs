//! Just enough of the Chrome DevTools protocol to show a page: start the browser, open a page
//! of the panel's size and receive its screencast.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

/// How long the browser may take to start (a first start on Windows, with the virus scanner
/// checking it, can take well over ten seconds).
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a command may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Start of the names of the throwaway profiles, followed by the daemon's process id.
const PROFILE_PREFIX: &str = "ssp-chrome-profile-";

/// A headless browser with a throwaway profile, closed (and its profile deleted) when dropped.
pub struct Browser {
    child: Child,
    profile: PathBuf,
    /// The DevTools WebSocket address of the browser.
    pub url: String,
}

impl Browser {
    /// Starts `chrome` with a window of `width` x `height`.
    pub fn launch(chrome: &Path, width: u32, height: u32) -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let profile = std::env::temp_dir().join(format!(
            "{PROFILE_PREFIX}{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)
            .map_err(|e| format!("cannot create {}: {e}", profile.display()))?;
        // What the browser reports goes to a file, to explain a failed start.
        let log = std::fs::File::create(profile.join("chrome.log")).map_err(|e| e.to_string())?;
        let mut command = Command::new(chrome);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg(format!("--window-size={width},{height}"))
            .args([
                "--force-device-scale-factor=1",
                "--hide-scrollbars",
                "--mute-audio",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-sync",
                "--disable-breakpad",
                "--disable-component-update",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                "--disable-backgrounding-occluded-windows",
                "about:blank",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let child = command
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", chrome.display()))?;
        let mut browser = Self {
            child,
            profile,
            url: String::new(),
        };
        browser.url = browser.wait_for_devtools()?;
        Ok(browser)
    }

    /// Reads the DevTools address the browser writes into its profile once it listens, or into
    /// its log: the Chromium snap of Ubuntu has a /tmp of its own, so the profile it writes to
    /// is not the one this process sees, but its error output still comes to our log file.
    fn wait_for_devtools(&mut self) -> Result<String, String> {
        let file = self.profile.join("DevToolsActivePort");
        let log = self.profile.join("chrome.log");
        let started = Instant::now();
        loop {
            if let Ok(text) = std::fs::read_to_string(&file) {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next())
                    && port.parse::<u16>().is_ok()
                {
                    return Ok(format!("ws://127.0.0.1:{port}{path}"));
                }
            }
            if let Some(url) = std::fs::read_to_string(&log)
                .ok()
                .and_then(|text| devtools_from_log(&text))
            {
                return Ok(url);
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                let log =
                    std::fs::read_to_string(self.profile.join("chrome.log")).unwrap_or_default();
                let reason = log
                    .lines()
                    .rfind(|l| !l.trim().is_empty())
                    .map_or_else(|| status.to_string(), |l| l.trim().to_owned());
                return Err(format!("the browser exited at start: {reason}"));
            }
            if started.elapsed() > START_TIMEOUT {
                return Err("the browser did not start in time".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The browser may still be letting go of files for a moment.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The address of "DevTools listening on ws://…" in what the browser printed.
fn devtools_from_log(log: &str) -> Option<String> {
    log.lines().find_map(|line| {
        let url = line.trim().strip_prefix("DevTools listening on ")?;
        url.starts_with("ws://").then(|| url.to_owned())
    })
}

/// Stops the browsers, and deletes the profiles, that daemons which did not exit cleanly (killed,
/// or crashed) left behind: a browser keeps running when the daemon that started it is gone.
pub fn clean_up_after_others() {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let profiles: Vec<(PathBuf, u32)> = entries
        .flatten()
        .filter_map(|entry| Some((entry.path(), owner(&entry.file_name().into_string().ok()?)?)))
        .filter(|(_, pid)| *pid != std::process::id())
        .collect();
    if profiles.is_empty() {
        return;
    }
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    for (profile, pid) in profiles {
        let daemon_runs = system
            .process(Pid::from_u32(pid))
            .is_some_and(|p| p.name().to_string_lossy().starts_with("ssp"));
        if daemon_runs {
            continue;
        }
        let flag = format!("--user-data-dir={}", profile.display());
        let mut stopped = 0;
        for process in system.processes().values() {
            if process
                .cmd()
                .iter()
                .any(|arg| arg.to_string_lossy() == flag)
                && process.kill()
            {
                stopped += 1;
            }
        }
        for _ in 0..20 {
            if std::fs::remove_dir_all(&profile).is_ok() || !profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        tracing::info!(
            profile = %profile.display(),
            "cleaned up after a daemon that did not exit cleanly ({stopped} browser processes)"
        );
    }
}

/// The daemon process id in a profile's name.
fn owner(name: &str) -> Option<u32> {
    name.strip_prefix(PROFILE_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// Something the browser reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// E.g. `"Page.screencastFrame"`.
    pub method: String,
    /// Its parameters.
    pub params: Value,
    /// The page session it belongs to, if any.
    pub session: Option<String>,
}

/// A DevTools connection.
pub struct Connection {
    socket: WebSocket<TcpStream>,
    next_id: u64,
    /// Events received while waiting for a reply.
    events: VecDeque<Event>,
}

impl Connection {
    /// Connects to a browser's DevTools address (`ws://127.0.0.1:<port>/...`).
    pub fn connect(url: &str) -> Result<Self, String> {
        let address = url
            .strip_prefix("ws://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| format!("unexpected DevTools address {url}"))?;
        let stream =
            TcpStream::connect(address).map_err(|e| format!("cannot reach the browser: {e}"))?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let (socket, _) = tungstenite::client(url, stream)
            .map_err(|e| format!("cannot open the DevTools connection: {e}"))?;
        Ok(Self {
            socket,
            next_id: 0,
            events: VecDeque::new(),
        })
    }

    /// Sends a command without waiting for its reply.
    pub fn send(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<u64, String> {
        self.next_id += 1;
        let mut message = json!({ "id": self.next_id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(message.to_string()))
            .map_err(|e| format!("the browser connection failed: {e}"))?;
        Ok(self.next_id)
    }

    /// Sends a command and waits for its result. Events that arrive meanwhile are kept for
    /// [`Connection::next_event`].
    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        let id = self.send(method, params, session)?;
        let started = Instant::now();
        loop {
            let remaining = CALL_TIMEOUT
                .checked_sub(started.elapsed())
                .ok_or_else(|| format!("{method}: no answer from the browser"))?;
            let Some(message) = self.read(remaining)? else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    let text = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("error");
                    return Err(format!("{method}: {text}"));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if let Some(event) = to_event(message) {
                self.events.push_back(event);
            }
        }
    }

    /// The next event, waiting at most `timeout`.
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Event>, String> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        Ok(self.read(timeout)?.and_then(to_event))
    }

    /// One message, or `None` if nothing arrives within `timeout`.
    fn read(&mut self, timeout: Duration) -> Result<Option<Value>, String> {
        let timeout = timeout.max(Duration::from_millis(1));
        self.socket
            .get_ref()
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(serde_json::from_str(text.as_str()).ok()),
            Ok(Message::Close(_)) => Err("the browser closed the connection".into()),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => Err(format!("the browser connection failed: {e}")),
        }
    }
}

fn to_event(message: Value) -> Option<Event> {
    let method = message.get("method")?.as_str()?.to_owned();
    Some(Event {
        method,
        session: message
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        params: message.get("params").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A browser left by a daemon that is gone is stopped and its profile deleted. Uses the
    /// browser in `SSP_TEST_CHROME`, if set.
    #[test]
    fn finds_the_devtools_address_in_the_log() {
        let log = "[1009/203100.1:WARNING] something\n\
                   \nDevTools listening on ws://127.0.0.1:41235/devtools/browser/0b1c-2d\n";
        assert_eq!(
            devtools_from_log(log).as_deref(),
            Some("ws://127.0.0.1:41235/devtools/browser/0b1c-2d")
        );
        assert_eq!(devtools_from_log("Fontconfig error\n"), None);
    }

    #[test]
    fn cleans_up_after_a_killed_daemon() {
        let Some(chrome) = std::env::var_os("SSP_TEST_CHROME") else {
            eprintln!("skipped: set SSP_TEST_CHROME to a Chrome to run it");
            return;
        };
        // No process has this id (process ids stay below 2^22 on Linux, 99999 on macOS).
        let profile = std::env::temp_dir().join(format!("{PROFILE_PREFIX}4999999-0"));
        let _ = std::fs::remove_dir_all(&profile);
        let mut browser = Command::new(chrome)
            .arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !profile.join("DevToolsActivePort").exists() && started.elapsed().as_secs() < 60 {
            std::thread::sleep(Duration::from_millis(100));
        }
        clean_up_after_others();
        let exited = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            browser.try_wait().unwrap().is_some()
        });
        if !exited {
            let _ = browser.kill();
        }
        assert!(exited, "the browser was stopped");
        assert!(!profile.exists(), "the profile was deleted");
    }

    #[test]
    fn reads_the_owner_of_a_profile() {
        assert_eq!(owner("ssp-chrome-profile-4242-0"), Some(4242));
        assert_eq!(owner("ssp-chrome-profile-x-0"), None);
        assert_eq!(owner("something-else"), None);
    }

    #[test]
    fn reads_events() {
        let event = to_event(json!({
            "method": "Page.screencastFrame",
            "params": {"sessionId": 3},
            "sessionId": "ABC"
        }))
        .unwrap();
        assert_eq!(event.method, "Page.screencastFrame");
        assert_eq!(event.session.as_deref(), Some("ABC"));
        assert_eq!(event.params["sessionId"], 3);
        assert!(to_event(json!({"id": 1, "result": {}})).is_none());
    }
}
