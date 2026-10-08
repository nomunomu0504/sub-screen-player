//! Keeps track of connected displays and what each one shows.
//!
//! The manager scans for devices every few seconds, opens new ones, notices unplugged ones and
//! remembers each display's [`Content`] across reconnects. Built-in sources run on a "player"
//! thread per display.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ssp_core::{
    Display, DisplayInfo, Found, Frame, Presenter, PresenterOptions, PresenterStats, Registry,
};

use crate::config::{Config, DisplayConfig, StartupShow};
use crate::metrics::Metrics;
use crate::sources::video::{self, VideoFile};
use crate::sources::{Clock, Content, Picture, Source};

/// How long to wait before retrying a device that failed to open.
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// Errors from looking up a display.
#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    /// No display has this id.
    #[error("no display {0:?}")]
    NotFound(String),
    /// The display is known but currently unplugged.
    #[error("display {0:?} is not connected")]
    NotConnected(String),
}

/// An open display.
pub struct Device {
    /// Feeds the display.
    pub presenter: Presenter,
    path: CString,
}

/// A snapshot of one display for the API.
#[derive(Debug, Clone)]
pub struct DisplayState {
    /// Stable id, e.g. `d92-470B03781D1F`.
    pub id: String,
    /// Last known identity and capabilities.
    pub info: DisplayInfo,
    /// Whether it is plugged in and open.
    pub connected: bool,
    /// What it is told to show (see [`Content::kind`]).
    pub content: &'static str,
    /// Counters of the current connection.
    pub stats: Option<PresenterStats>,
}

/// Proof that a WebSocket stream owns a display; lost when other content is set.
pub struct StreamTicket {
    /// The resolved display id.
    pub id: String,
    generation: Arc<AtomicU64>,
    mine: u64,
}

impl StreamTicket {
    /// `false` once other content replaced the stream.
    pub fn is_current(&self) -> bool {
        self.generation.load(Ordering::SeqCst) == self.mine
    }
}

/// See the module documentation.
pub struct Manager {
    registry: Registry,
    display: DisplayConfig,
    startup: Content,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<String, Entry>,
    failed: HashMap<CString, Instant>,
}

struct Entry {
    info: DisplayInfo,
    device: Option<Arc<Device>>,
    content: Content,
    /// Bumped whenever the content changes, so stale players and streams stop.
    generation: Arc<AtomicU64>,
    player: Option<Player>,
}

impl Manager {
    /// Creates a manager. Fails if the startup content cannot be loaded. Dashboards read
    /// `metrics`.
    pub fn new(registry: Registry, config: &Config, metrics: Metrics) -> Result<Self, String> {
        Ok(Self {
            registry,
            display: config.display.clone(),
            startup: startup_content(config, metrics)?,
            inner: Mutex::default(),
        })
    }

    /// Ids of the drivers in use.
    pub fn drivers(&self) -> Vec<&'static str> {
        self.registry.enabled().map(|d| d.id()).collect()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One hotplug pass: forgets unplugged devices and opens new ones.
    pub fn scan(&self) {
        let mut stale = Vec::new();
        for (id, entry) in &mut self.lock().entries {
            if entry
                .device
                .as_ref()
                .is_some_and(|d| !d.presenter.is_running())
            {
                let device = entry.device.take().expect("checked above");
                let reason = device
                    .presenter
                    .failure()
                    .unwrap_or_else(|| "closed".into());
                tracing::info!(display = %id, "disconnected: {reason}");
                stale.extend(entry.player.take());
            }
        }
        stale.into_iter().for_each(Player::stop);

        let found = match self.registry.scan() {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!("device scan failed: {err}");
                return;
            }
        };
        for Found { driver, candidate } in found {
            let first_failure = {
                let inner = self.lock();
                let open = inner
                    .entries
                    .values()
                    .any(|e| e.device.as_ref().is_some_and(|d| d.path == candidate.path));
                match inner.failed.get(&candidate.path) {
                    _ if open => continue,
                    Some(at) if at.elapsed() < RETRY_AFTER => continue,
                    failed => failed.is_none(),
                }
            };
            match driver.open(&candidate) {
                Ok(display) => {
                    self.lock().failed.remove(&candidate.path);
                    self.attach(display, candidate.path);
                }
                Err(err) => {
                    let message = format!("cannot open {} display: {err}", driver.name());
                    if first_failure {
                        tracing::warn!("{message}");
                    } else {
                        tracing::debug!("{message}");
                    }
                    self.lock().failed.insert(candidate.path, Instant::now());
                }
            }
        }
    }

    /// Takes over an opened display and starts its content. `path` identifies the interface
    /// so it is not opened twice.
    pub fn attach(&self, display: Box<dyn Display>, path: CString) {
        let info = display.info().clone();
        let options = PresenterOptions {
            max_fps: self.display.max_fps,
            quality: self.display.quality,
            min_quality: self.display.min_quality,
            skip_duplicates: true,
        };
        let presenter = Presenter::spawn(display, options);
        if let Some(percent) = self.display.brightness
            && let Err(err) = presenter.set_brightness(percent)
        {
            tracing::warn!(display = %info.id(), "cannot set brightness: {err}");
        }
        let device = Arc::new(Device { presenter, path });

        let mut inner = self.lock();
        // Two devices without serial numbers share an id; number the later ones.
        let base = info.id();
        let mut id = base.clone();
        for n in 2.. {
            match inner.entries.get(&id) {
                Some(entry) if entry.device.is_some() => id = format!("{base}-{n}"),
                _ => break,
            }
        }
        tracing::info!(
            display = %id,
            model = %info.model,
            firmware = info.firmware.as_deref().unwrap_or("unknown"),
            "connected"
        );
        let entry = inner.entries.entry(id).or_insert_with(|| Entry {
            info: info.clone(),
            device: None,
            content: self.startup.clone(),
            generation: Arc::default(),
            player: None,
        });
        entry.info = info;
        entry.device = Some(device);
        entry.generation.fetch_add(1, Ordering::SeqCst);
        start(entry);
    }

    /// Runs [`Manager::scan`] every `every` on a background thread.
    pub fn spawn_scanner(self: &Arc<Self>, every: Duration) -> Scanner {
        let manager = self.clone();
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("ssp-scan".into())
            .spawn(move || {
                loop {
                    manager.scan();
                    if stopped.recv_timeout(every) != Err(mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            })
            .expect("failed to spawn a thread");
        Scanner { stop, thread }
    }

    /// All displays seen since the daemon started.
    pub fn displays(&self) -> Vec<DisplayState> {
        self.lock()
            .entries
            .iter()
            .map(|(id, entry)| DisplayState {
                id: id.clone(),
                info: entry.info.clone(),
                connected: entry.device.is_some(),
                content: entry.content.kind(),
                stats: entry.device.as_ref().map(|d| d.presenter.stats()),
            })
            .collect()
    }

    /// One display. `"default"` means the first connected one.
    pub fn display(&self, id: &str) -> Result<DisplayState, LookupError> {
        let id = self.resolve(&self.lock(), id)?;
        Ok(self
            .displays()
            .into_iter()
            .find(|d| d.id == id)
            .expect("resolved above"))
    }

    /// The open device behind `id` (`"default"`: the first connected one).
    pub fn device(&self, id: &str) -> Result<Arc<Device>, LookupError> {
        let inner = self.lock();
        let id = self.resolve(&inner, id)?;
        inner.entries[&id]
            .device
            .clone()
            .ok_or(LookupError::NotConnected(id))
    }

    fn resolve(&self, inner: &Inner, id: &str) -> Result<String, LookupError> {
        if id == "default" {
            return inner
                .entries
                .iter()
                .find(|(_, e)| e.device.is_some())
                .map(|(id, _)| id.clone())
                .ok_or_else(|| LookupError::NotConnected(id.into()));
        }
        if inner.entries.contains_key(id) {
            Ok(id.to_string())
        } else {
            Err(LookupError::NotFound(id.into()))
        }
    }

    /// Changes what a display shows. Kept across reconnects.
    pub fn set_content(&self, id: &str, content: Content) -> Result<(), LookupError> {
        self.replace_content(id, content).map(|_| ())
    }

    /// Hands the display to a WebSocket stream.
    pub fn begin_stream(&self, id: &str) -> Result<StreamTicket, LookupError> {
        self.replace_content(id, Content::Stream)
    }

    fn replace_content(&self, id: &str, content: Content) -> Result<StreamTicket, LookupError> {
        let (ticket, old) = {
            let mut inner = self.lock();
            let id = self.resolve(&inner, id)?;
            let entry = inner.entries.get_mut(&id).expect("resolved above");
            let old = entry.player.take();
            entry.content = content;
            let mine = entry.generation.fetch_add(1, Ordering::SeqCst) + 1;
            start(entry);
            (
                StreamTicket {
                    id,
                    generation: entry.generation.clone(),
                    mine,
                },
                old,
            )
        };
        if let Some(old) = old {
            old.stop();
        }
        Ok(ticket)
    }

    /// Stops all sources and closes every display with the configured exit action.
    pub fn shutdown(&self) {
        let (players, devices): (Vec<_>, Vec<_>) = self
            .lock()
            .entries
            .values_mut()
            .map(|e| (e.player.take(), e.device.take()))
            .unzip();
        players.into_iter().flatten().for_each(Player::stop);
        for device in devices.into_iter().flatten() {
            if let Err(err) = device.presenter.stop(self.display.on_exit.into()) {
                tracing::warn!(display = %device.presenter.info().id(), "on exit: {err}");
            }
        }
    }
}

/// Starts the entry's content on its device, if both exist.
fn start(entry: &mut Entry) {
    let (Some(device), Some(source)) = (&entry.device, entry.content.source()) else {
        return;
    };
    let generation = entry.generation.clone();
    let mine = generation.load(Ordering::SeqCst);
    entry.player = Some(Player::start(device.clone(), source, generation, mine));
}

fn startup_content(config: &Config, metrics: Metrics) -> Result<Content, String> {
    match config.startup.show {
        StartupShow::Nothing => Ok(Content::Nothing),
        StartupShow::Clock => {
            Clock::validate(&config.clock)?;
            Ok(Content::Clock(config.clock.clone()))
        }
        StartupShow::Dashboard => {
            Clock::validate(&config.clock)?;
            config.dashboard.validate()?;
            Ok(Content::Dashboard(
                config.dashboard.clone(),
                config.clock.clone(),
                metrics,
            ))
        }
        StartupShow::Image => {
            let path = config
                .startup
                .image
                .as_ref()
                .ok_or("startup.image is not set")?;
            let fit = config.startup.fit.into();
            if starts_like_a_video(path) {
                let ffmpeg = video::find_ffmpeg(config.video.ffmpeg.as_deref())?;
                let video = VideoFile::open(path.clone(), ffmpeg, false)
                    .map_err(|e| format!("startup video {}: {e}", path.display()))?;
                return Ok(Content::Video {
                    video: Arc::new(video),
                    fit,
                });
            }
            let bytes = std::fs::read(path)
                .map_err(|e| format!("cannot read startup image {}: {e}", path.display()))?;
            let picture = Picture::decode(&bytes)
                .map_err(|e| format!("startup image {}: {e}", path.display()))?;
            Ok(picture.into_content(fit))
        }
    }
}

/// Whether the file at `path` starts like a video (see [`video::is_video`]).
fn starts_like_a_video(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(256).read_to_end(&mut head))
        .is_ok_and(|_| video::is_video(&head) && image::guess_format(&head).is_err())
}

/// The background thread of [`Manager::spawn_scanner`].
pub struct Scanner {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Scanner {
    /// Stops scanning and waits for a running scan to finish.
    pub fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}

/// Runs a [`Source`] for one device until stopped or superseded.
struct Player {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Player {
    fn start(
        device: Arc<Device>,
        mut source: Box<dyn Source>,
        generation: Arc<AtomicU64>,
        mine: u64,
    ) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let name = format!("ssp-player-{}", device.presenter.info().id());
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || {
                let panel = device.presenter.info().panel;
                loop {
                    let mut frame = Frame::blank(panel.width, panel.height);
                    source.render(&mut frame);
                    if generation.load(Ordering::SeqCst) != mine {
                        break;
                    }
                    if let Err(err) = device.presenter.submit(frame) {
                        tracing::debug!("source stopped: {err}");
                        break;
                    }
                    let Some(wait) = source.next_change() else {
                        break;
                    };
                    if stopped.recv_timeout(wait) != Err(mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            })
            .expect("failed to spawn a thread");
        Self { stop, thread }
    }

    fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClockConfig;
    use ssp_core::testing::{Call, CallLog, FakeDisplay};

    fn manager(show: StartupShow) -> Manager {
        let mut config = Config::default();
        config.startup.show = show;
        config.display.brightness = Some(42);
        Manager::new(Registry::new(), &config, Metrics::default()).unwrap()
    }

    fn attach(manager: &Manager, path: &str) -> (CallLog, ssp_core::testing::Unplug) {
        let (display, log) = FakeDisplay::new();
        let unplug = display.unplug_handle();
        manager.attach(Box::new(display), CString::new(path).unwrap());
        (log, unplug)
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn new_display_gets_brightness_and_startup_clock() {
        let manager = manager(StartupShow::Clock);
        let (log, _) = attach(&manager, "a");
        wait_until(|| log.calls().iter().any(|c| matches!(c, Call::Show(_))));
        assert_eq!(log.calls()[0], Call::Brightness(42));
        let displays = manager.displays();
        assert_eq!(displays.len(), 1);
        assert_eq!(displays[0].id, "fake-0001");
        assert_eq!(displays[0].content, "clock");
        assert!(displays[0].connected);
        manager.shutdown();
    }

    #[test]
    fn default_resolves_to_the_first_connected_display() {
        let manager = manager(StartupShow::Nothing);
        assert!(matches!(
            manager.device("default"),
            Err(LookupError::NotConnected(_))
        ));
        attach(&manager, "a");
        assert_eq!(manager.display("default").unwrap().id, "fake-0001");
        assert!(matches!(
            manager.device("nope"),
            Err(LookupError::NotFound(_))
        ));
        manager.shutdown();
    }

    #[test]
    fn displays_with_the_same_id_are_numbered() {
        let manager = manager(StartupShow::Nothing);
        attach(&manager, "a");
        attach(&manager, "b");
        let ids: Vec<String> = manager.displays().into_iter().map(|d| d.id).collect();
        assert_eq!(ids, ["fake-0001", "fake-0001-2"]);
        manager.shutdown();
    }

    #[test]
    fn content_survives_a_reconnect() {
        let manager = manager(StartupShow::Nothing);
        let (_, unplug) = attach(&manager, "a");
        manager
            .set_content("default", Content::Clock(ClockConfig::default()))
            .unwrap();
        unplug.unplug();
        wait_until(|| !manager.device("fake-0001").unwrap().presenter.is_running());
        manager.scan();
        assert!(!manager.displays()[0].connected);
        let (log, _) = attach(&manager, "a2");
        wait_until(|| log.calls().iter().any(|c| matches!(c, Call::Show(_))));
        assert_eq!(manager.displays()[0].content, "clock");
        manager.shutdown();
    }

    #[test]
    fn new_content_invalidates_a_stream() {
        let manager = manager(StartupShow::Nothing);
        attach(&manager, "a");
        let ticket = manager.begin_stream("default").unwrap();
        assert!(ticket.is_current());
        manager.set_content("default", Content::Nothing).unwrap();
        assert!(!ticket.is_current());
        manager.shutdown();
    }

    #[test]
    fn plays_a_startup_video() {
        let Ok(ffmpeg) = video::find_ffmpeg(None) else {
            eprintln!("skipped: ffmpeg is not installed");
            return;
        };
        let path = std::env::temp_dir().join(format!("ssp-startup-{}.mp4", std::process::id()));
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
            .arg(&path)
            .status()
            .unwrap();
        assert!(made.success());
        let mut config = Config::default();
        config.startup.show = StartupShow::Image;
        config.startup.image = Some(path.clone());
        let content = startup_content(&config, Metrics::default()).unwrap();
        assert_eq!(content.kind(), "video");
        drop(content);
        // A file named in the config is never deleted.
        assert!(path.exists());
        let _ = std::fs::remove_file(path);
    }
}
