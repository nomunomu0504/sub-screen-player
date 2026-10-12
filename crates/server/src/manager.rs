//! Keeps track of connected displays and what each one shows.
//!
//! The manager scans for devices every few seconds, opens new ones, notices unplugged ones and
//! remembers each display's [`Content`] across reconnects. Built-in sources run on a "player"
//! thread per display. A change for a while ([`Manager::choose_for`]) ends on a timer thread.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use ssp_core::{
    Display, DisplayInfo, Found, Frame, Presenter, PresenterOptions, PresenterStats, Registry,
    StopAction,
};

use crate::config::{Config, DisplayConfig};
use crate::metrics::Metrics;
use crate::notify::{Notification, Overlay, Shown};
use crate::schedule::{Schedule, State};
use crate::sources::{Content, Feed, Source};

/// How long to wait before retrying a device that failed to open.
const RETRY_AFTER: Duration = Duration::from_secs(10);

/// The timer looks at the clock at least this often, so that it notices a computer that slept.
const TIMER_MAX_WAIT: Duration = Duration::from_secs(60);

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
    /// Feeds the display. Frames of contents go through [`Device::submit`] instead, so the
    /// notification is drawn over them.
    pub presenter: Presenter,
    path: CString,
    overlay: Arc<Overlay>,
}

impl Device {
    /// Queues a frame of the display's content, with the notification (if any) drawn on top.
    pub fn submit(&self, frame: Frame) -> ssp_core::Result<()> {
        self.presenter.submit(self.overlay.pass(frame))
    }

    /// Blanks the screen and ends the notification without showing anything else.
    pub fn clear(&self) -> ssp_core::Result<()> {
        self.overlay.forget();
        self.presenter.clear()
    }
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
    /// The notification being shown.
    pub notification: Option<Shown>,
    /// When a change for a while ends.
    pub content_until: Option<Timestamp>,
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
    /// Decides what connecting displays show, if the config has a schedule.
    schedule: Mutex<Option<Arc<Schedule>>>,
    /// What displays were told to show before the config was reloaded, by display id: they
    /// show it again when they connect.
    restored: Mutex<HashMap<String, Restored>>,
    inner: Mutex<Inner>,
    /// Wakes the timer when a change for a while starts, and when it is stopped.
    timer: Condvar,
}

/// Builds content with the settings of a config, for content chosen through the API that is
/// shown again after the config is reloaded.
pub type Rebuild = Arc<dyn Fn(&Config, &Metrics) -> Result<Content, String> + Send + Sync>;

/// What a display was told to show through the API (not by the startup setting or the
/// schedule), kept when the config is reloaded.
#[derive(Clone)]
pub enum Chosen {
    /// Built again with the new config: the clock, the dashboard, a web page, a layout.
    Rebuild(Rebuild),
    /// Shown again as it is: pictures and videos sent to the API, or nothing.
    Same(Box<Content>),
}

impl Chosen {
    /// `content`, shown again as it is.
    pub fn same(content: Content) -> Self {
        Self::Same(Box::new(content))
    }

    /// The content to show with `config`.
    pub fn content(&self, config: &Config, metrics: &Metrics) -> Result<Content, String> {
        match self {
            Self::Rebuild(build) => build(config, metrics),
            Self::Same(content) => Ok((**content).clone()),
        }
    }
}

/// What a display was told to show through the API, kept across a reload of the config
/// ([`Manager::chosen`], [`Manager::restore`]).
#[derive(Clone)]
pub struct Kept {
    chosen: Chosen,
    /// For a change for a while: when it ends, and what was chosen before it (`None`: the
    /// schedule's or the startup content).
    timed: Option<(Timestamp, Option<Chosen>)>,
}

impl Kept {
    /// How the content was chosen.
    pub fn chosen(&self) -> &Chosen {
        &self.chosen
    }
}

impl From<Chosen> for Kept {
    fn from(chosen: Chosen) -> Self {
        Self {
            chosen,
            timed: None,
        }
    }
}

/// [`Kept`] built with the new config, until its display connects.
struct Restored {
    content: Content,
    chosen: Chosen,
    /// When a change for a while ends, and what to show then (`None`: the schedule's or the
    /// startup content).
    timed: Option<(Timestamp, Option<(Content, Chosen)>)>,
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<String, Entry>,
    failed: HashMap<CString, Instant>,
    timer_stopped: bool,
}

struct Entry {
    info: DisplayInfo,
    device: Option<Arc<Device>>,
    content: Content,
    /// How the content was chosen through the API, if it was.
    chosen: Option<Chosen>,
    /// Bumped whenever the content changes, so stale players and streams stop.
    generation: Arc<AtomicU64>,
    player: Option<Player>,
    /// The notification, kept across reconnects.
    overlay: Arc<Overlay>,
    /// What to show again when a change for a while ends.
    timed: Option<Timed>,
}

/// A change for a while: when it ends, and what the display showed before.
struct Timed {
    until: Timestamp,
    content: Content,
    chosen: Option<Chosen>,
}

impl Manager {
    /// Creates a manager. Fails if the startup content cannot be loaded. Dashboards read
    /// `metrics`.
    pub fn new(registry: Registry, config: &Config, metrics: Metrics) -> Result<Self, String> {
        Ok(Self {
            registry,
            display: config.display.clone(),
            startup: startup_content(config, metrics)?,
            schedule: Mutex::default(),
            restored: Mutex::default(),
            inner: Mutex::default(),
            timer: Condvar::new(),
        })
    }

    /// What displays were told to show through the API, to show again after a reload.
    pub fn chosen(&self) -> Vec<(String, Kept)> {
        self.lock()
            .entries
            .iter()
            .filter_map(|(id, e)| {
                let kept = Kept {
                    chosen: e.chosen.clone()?,
                    timed: e.timed.as_ref().map(|t| (t.until, t.chosen.clone())),
                };
                Some((id.clone(), kept))
            })
            .collect()
    }

    /// Has displays show again what they were told to before the config was reloaded, built
    /// with `config`, when they connect; a change for a while still ends when it would have.
    /// What cannot be built any more (a file that is gone) gives way to the schedule's or the
    /// startup content.
    pub fn restore(&self, kept: Vec<(String, Kept)>, config: &Config, metrics: &Metrics) {
        let build = |id: &str, chosen: &Chosen| match chosen.content(config, metrics) {
            Ok(content) => Some(content),
            Err(err) => {
                tracing::warn!(display = %id, "cannot show again what it showed: {err}");
                None
            }
        };
        let mut restored = self.restored.lock().unwrap_or_else(|p| p.into_inner());
        for (id, kept) in kept {
            let Some(content) = build(&id, &kept.chosen) else {
                continue;
            };
            let timed = kept.timed.map(|(until, before)| {
                let before = before.and_then(|chosen| Some((build(&id, &chosen)?, chosen)));
                (until, before)
            });
            restored.insert(
                id,
                Restored {
                    content,
                    chosen: kept.chosen,
                    timed,
                },
            );
        }
    }

    /// Lets `schedule` decide what displays show, how bright they are and whether their screens
    /// are on when they connect.
    pub fn set_schedule(&self, schedule: Arc<Schedule>) {
        *self.schedule.lock().unwrap_or_else(|p| p.into_inner()) = Some(schedule);
    }

    /// The schedule, if the config has one.
    pub fn schedule(&self) -> Option<Arc<Schedule>> {
        self.schedule
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Ids of all displays seen since the daemon started.
    pub fn ids(&self) -> Vec<String> {
        self.lock().entries.keys().cloned().collect()
    }

    /// Applies what a schedule says: shows its content (also on an unplugged display, which
    /// shows it when it is back), and sets the brightness and the screen of a connected one.
    pub fn apply(&self, id: &str, state: &State) {
        if let Some(content) = &state.show
            && let Err(err) = self.set_content(id, content.clone())
        {
            tracing::warn!(display = %id, "schedule: {err}");
        }
        let Ok(device) = self.device(id) else {
            return;
        };
        set_screen(&device.presenter, id, state.brightness, state.power);
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
            partial_updates: self.display.partial_updates,
        };
        let presenter = Presenter::spawn(display, options);
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
        let state = self
            .schedule()
            .map(|s| s.state(&id, &jiff::Zoned::now()))
            .unwrap_or_default();
        let brightness = state.brightness.or(self.display.brightness);
        // A display seen before carries on with its content; a new one starts with what it
        // showed before the config was reloaded, else the schedule's, else the startup content.
        let restored = self
            .restored
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
        let entry = inner.entries.entry(id.clone()).or_insert_with(|| {
            let planned = || state.show.clone().unwrap_or_else(|| self.startup.clone());
            let (content, chosen, timed) = match restored {
                Some(restored) => {
                    let timed = restored.timed.map(|(until, before)| {
                        let (content, chosen) = match before {
                            Some((content, chosen)) => (content, Some(chosen)),
                            None => (planned(), None),
                        };
                        Timed {
                            until,
                            content,
                            chosen,
                        }
                    });
                    (restored.content, Some(restored.chosen), timed)
                }
                None => (planned(), None, None),
            };
            Entry {
                info: info.clone(),
                device: None,
                content,
                chosen,
                generation: Arc::default(),
                player: None,
                overlay: Overlay::new(),
                timed,
            }
        });
        let device = Arc::new(Device {
            presenter,
            path,
            overlay: entry.overlay.clone(),
        });
        entry.overlay.attach(&device);
        entry.info = info;
        entry.device = Some(device.clone());
        // The content stays the same: a stream keeps its ticket. The player of the device before
        // was stopped when it was found gone.
        start(entry);
        let timed = entry.timed.is_some();
        drop(inner);
        if timed {
            self.timer.notify_all();
        }
        set_screen(&device.presenter, &id, brightness, state.power);
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
                notification: entry.overlay.current(),
                content_until: entry.timed.as_ref().map(|t| t.until),
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

    /// Changes what a display shows (for the schedule). Kept across reconnects.
    pub fn set_content(&self, id: &str, content: Content) -> Result<(), LookupError> {
        self.replace_content(id, content, None, None).map(|_| ())
    }

    /// Changes what a display shows, as told through the API. Kept across reconnects, and
    /// across reloads of the config as `chosen` says.
    pub fn choose(&self, id: &str, content: Content, chosen: Chosen) -> Result<(), LookupError> {
        self.choose_for(id, content, chosen, None)
    }

    /// [`Manager::choose`], for a while if `time` is given: then the display shows again what it
    /// showed before (before the first of several such changes in a row), unless something else
    /// changed what it shows by then. Needs [`Manager::spawn_timer`].
    pub fn choose_for(
        &self,
        id: &str,
        content: Content,
        chosen: Chosen,
        time: Option<Duration>,
    ) -> Result<(), LookupError> {
        let until = time.map(|time| Timestamp::now().checked_add(time).unwrap_or(Timestamp::MAX));
        self.replace_content(id, content, Some(chosen), until)
            .map(|_| ())
    }

    /// Shows `notification` over what the display shows, in place of the current one. A
    /// display that is not connected shows it when it connects, if it has not ended by then.
    pub fn notify(&self, id: &str, notification: Notification) -> Result<(), LookupError> {
        let overlay = {
            let inner = self.lock();
            let id = self.resolve(&inner, id)?;
            inner.entries[&id].overlay.clone()
        };
        overlay.show(notification);
        Ok(())
    }

    /// Ends the display's notification. `false` if there was none.
    pub fn dismiss(&self, id: &str) -> Result<bool, LookupError> {
        let overlay = {
            let inner = self.lock();
            let id = self.resolve(&inner, id)?;
            inner.entries[&id].overlay.clone()
        };
        Ok(overlay.dismiss())
    }

    /// Hands the display to a WebSocket stream.
    pub fn begin_stream(&self, id: &str) -> Result<StreamTicket, LookupError> {
        self.replace_content(id, Content::Stream, None, None)
    }

    /// The `stream` zone of the layout the display shows, for a WebSocket stream, with a ticket
    /// that is lost when the display shows something else. `None` if it shows no layout with
    /// one.
    pub fn stream_zone(&self, id: &str) -> Result<Option<(StreamTicket, Arc<Feed>)>, LookupError> {
        let inner = self.lock();
        let id = self.resolve(&inner, id)?;
        let entry = &inner.entries[&id];
        let Content::Layout(spec) = &entry.content else {
            return Ok(None);
        };
        let Some(feed) = spec.feed.clone() else {
            return Ok(None);
        };
        let ticket = StreamTicket {
            mine: entry.generation.load(Ordering::SeqCst),
            generation: entry.generation.clone(),
            id,
        };
        Ok(Some((ticket, feed)))
    }

    fn replace_content(
        &self,
        id: &str,
        content: Content,
        chosen: Option<Chosen>,
        until: Option<Timestamp>,
    ) -> Result<StreamTicket, LookupError> {
        let (ticket, old) = {
            let mut inner = self.lock();
            let id = self.resolve(&inner, id)?;
            let entry = inner.entries.get_mut(&id).expect("resolved above");
            let earlier = entry.timed.take();
            entry.timed = until.map(|until| match earlier {
                Some(earlier) => Timed { until, ..earlier },
                None => Timed {
                    until,
                    content: entry.content.clone(),
                    chosen: entry.chosen.clone(),
                },
            });
            let old = swap(entry, content, chosen);
            (
                StreamTicket {
                    id,
                    generation: entry.generation.clone(),
                    mine: entry.generation.load(Ordering::SeqCst),
                },
                old,
            )
        };
        if until.is_some() {
            self.timer.notify_all();
        }
        if let Some(old) = old {
            old.stop();
        }
        Ok(ticket)
    }

    /// Ends changes for a while when their time comes, on a thread of its own, until
    /// [`Timer::stop`].
    pub fn spawn_timer(self: &Arc<Self>) -> Timer {
        let manager = self.clone();
        let thread = std::thread::Builder::new()
            .name("ssp-timer".into())
            .spawn(move || manager.end_changes())
            .expect("failed to spawn a thread");
        Timer {
            manager: self.clone(),
            thread,
        }
    }

    fn end_changes(&self) {
        let mut inner = self.lock();
        while !inner.timer_stopped {
            let now = Timestamp::now();
            let mut old = Vec::new();
            for (id, entry) in &mut inner.entries {
                if let Some(timed) = entry.timed.take_if(|t| t.until <= now) {
                    tracing::info!(display = %id, "back to {}", timed.content.kind());
                    old.extend(swap(entry, timed.content, timed.chosen));
                }
            }
            if !old.is_empty() {
                drop(inner);
                old.into_iter().for_each(Player::stop);
                inner = self.lock();
                continue;
            }
            let next = inner
                .entries
                .values()
                .filter_map(|e| Some(e.timed.as_ref()?.until))
                .min();
            let wait = next.map_or(TIMER_MAX_WAIT, |until| {
                Duration::try_from(until.duration_since(now))
                    .unwrap_or_default()
                    .min(TIMER_MAX_WAIT)
            });
            inner = self
                .timer
                .wait_timeout(inner, wait)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Stops all sources and closes every display with the configured exit action.
    pub fn shutdown(&self) {
        self.stop_all(self.display.on_exit.into());
    }

    /// Stops all sources and closes every display, leaving its screen as it is: the config is
    /// being reloaded, and the displays open again right after.
    pub fn close(&self) {
        self.stop_all(StopAction::Leave);
    }

    fn stop_all(&self, action: StopAction) {
        let (players, devices): (Vec<_>, Vec<_>) = self
            .lock()
            .entries
            .values_mut()
            .map(|e| (e.player.take(), e.device.take()))
            .unzip();
        players.into_iter().flatten().for_each(Player::stop);
        for device in devices.into_iter().flatten() {
            if let Err(err) = device.presenter.stop(action) {
                tracing::warn!(display = %device.presenter.info().id(), "on exit: {err}");
            }
        }
    }
}

/// Sets the backlight and switches the screen on or off, as far as they are given.
fn set_screen(presenter: &Presenter, id: &str, brightness: Option<u8>, on: Option<bool>) {
    if let Some(percent) = brightness
        && let Err(err) = presenter.set_brightness(percent)
    {
        tracing::warn!(display = %id, "cannot set brightness: {err}");
    }
    let result = match on {
        Some(true) if presenter.is_asleep() => presenter.wake(),
        Some(false) if !presenter.is_asleep() => presenter.sleep(),
        _ => Ok(()),
    };
    if let Err(err) = result {
        tracing::warn!(display = %id, "cannot switch the screen: {err}");
    }
}

/// Has the entry show `content` in place of what it showed. Returns the player to stop, once the
/// manager is unlocked.
fn swap(entry: &mut Entry, content: Content, chosen: Option<Chosen>) -> Option<Player> {
    let old = entry.player.take();
    entry.content = content;
    entry.chosen = chosen;
    entry.generation.fetch_add(1, Ordering::SeqCst);
    start(entry);
    old
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
    Content::from_spec(config.startup.spec(), config, &metrics).map_err(|e| format!("startup: {e}"))
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

/// The thread of [`Manager::spawn_timer`].
pub struct Timer {
    manager: Arc<Manager>,
    thread: JoinHandle<()>,
}

impl Timer {
    /// Stops ending changes and waits for the thread to end.
    pub fn stop(self) {
        self.manager.lock().timer_stopped = true;
        self.manager.timer.notify_all();
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
                    if let Err(err) = device.submit(frame) {
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
    use crate::config::{ClockConfig, StartupShow};
    use crate::sources::video;
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
    fn what_the_api_chose_is_shown_again_after_a_reload() {
        let manager = manager(StartupShow::Clock);
        let (_, _) = attach(&manager, "a");
        assert!(
            manager.chosen().is_empty(),
            "the startup content was not chosen"
        );
        let chosen = Chosen::same(Content::Nothing);
        manager
            .choose("fake-0001", Content::Nothing, chosen.clone())
            .unwrap();
        assert_eq!(manager.chosen().len(), 1);
        // The schedule takes over from what was chosen.
        manager.set_content("fake-0001", Content::Nothing).unwrap();
        assert!(manager.chosen().is_empty());
        manager
            .choose("fake-0001", Content::Nothing, chosen)
            .unwrap();
        let carried = manager.chosen();
        manager.close();

        // The daemon again, with the new config: the display shows what was chosen, not the
        // startup clock, and it is still chosen.
        let again = Manager::new(Registry::new(), &Config::default(), Metrics::default()).unwrap();
        again.restore(carried, &Config::default(), &Metrics::default());
        attach(&again, "a");
        assert_eq!(again.display("fake-0001").unwrap().content, "nothing");
        assert_eq!(again.chosen().len(), 1);
        // What cannot be built any more gives way to the startup content.
        let gone: Rebuild = Arc::new(|_: &Config, _: &Metrics| Err("gone".to_owned()));
        let third = Manager::new(Registry::new(), &Config::default(), Metrics::default()).unwrap();
        third.restore(
            vec![("fake-0001".into(), Chosen::Rebuild(gone).into())],
            &Config::default(),
            &Metrics::default(),
        );
        attach(&third, "a");
        assert_eq!(third.display("fake-0001").unwrap().content, "clock");
        again.shutdown();
        third.shutdown();
    }

    fn dashboard() -> Content {
        Content::Dashboard(
            crate::config::DashboardConfig::default(),
            ClockConfig::default(),
            Metrics::default(),
        )
    }

    fn shows(manager: &Manager) -> &'static str {
        manager.display("fake-0001").unwrap().content
    }

    #[test]
    fn a_change_for_a_while_goes_back() {
        let manager = Arc::new(manager(StartupShow::Clock));
        let timer = manager.spawn_timer();
        attach(&manager, "a");
        let short = Some(Duration::from_secs(1));
        manager
            .choose_for(
                "default",
                Content::Nothing,
                Chosen::same(Content::Nothing),
                short,
            )
            .unwrap();
        assert_eq!(shows(&manager), "nothing");
        assert!(manager.display("default").unwrap().content_until.is_some());
        wait_until(|| shows(&manager) == "clock");
        let state = manager.display("default").unwrap();
        assert_eq!(state.content_until, None);
        assert!(manager.chosen().is_empty(), "the startup content again");

        // Several in a row go back to what showed before the first.
        manager
            .choose("default", Content::Nothing, Chosen::same(Content::Nothing))
            .unwrap();
        let long = Some(Duration::from_secs(60));
        manager
            .choose_for("default", dashboard(), Chosen::same(dashboard()), long)
            .unwrap();
        manager
            .choose_for(
                "default",
                Content::Clock(ClockConfig::default()),
                Chosen::same(Content::Clock(ClockConfig::default())),
                short,
            )
            .unwrap();
        wait_until(|| shows(&manager) == "nothing");
        assert_eq!(manager.chosen().len(), 1, "chosen through the API again");
        timer.stop();
        manager.shutdown();
    }

    #[test]
    fn another_change_ends_a_change_for_a_while() {
        let manager = Arc::new(manager(StartupShow::Clock));
        let timer = manager.spawn_timer();
        attach(&manager, "a");
        let short = Some(Duration::from_millis(100));
        manager
            .choose_for(
                "default",
                Content::Nothing,
                Chosen::same(Content::Nothing),
                short,
            )
            .unwrap();
        // The schedule's next entry.
        manager.set_content("default", dashboard()).unwrap();
        assert_eq!(manager.display("default").unwrap().content_until, None);
        manager
            .choose_for(
                "default",
                Content::Nothing,
                Chosen::same(Content::Nothing),
                short,
            )
            .unwrap();
        let ticket = manager.begin_stream("default").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(shows(&manager), "stream");
        assert!(ticket.is_current());
        timer.stop();
        manager.shutdown();
    }

    #[test]
    fn a_change_for_a_while_lasts_through_a_reload() {
        let manager = manager(StartupShow::Clock);
        attach(&manager, "a");
        manager
            .choose("default", Content::Nothing, Chosen::same(Content::Nothing))
            .unwrap();
        let time = Some(Duration::from_secs(2));
        manager
            .choose_for("default", dashboard(), Chosen::same(dashboard()), time)
            .unwrap();
        let until = manager.display("default").unwrap().content_until;
        let kept = manager.chosen();
        manager.close();

        let again = Arc::new(
            Manager::new(Registry::new(), &Config::default(), Metrics::default()).unwrap(),
        );
        again.restore(kept, &Config::default(), &Metrics::default());
        let timer = again.spawn_timer();
        attach(&again, "a");
        assert_eq!(shows(&again), "dashboard");
        assert_eq!(again.display("default").unwrap().content_until, until);
        wait_until(|| shows(&again) == "nothing");
        assert_eq!(again.chosen().len(), 1);
        timer.stop();
        again.shutdown();
    }

    #[test]
    fn closing_for_a_reload_leaves_the_screen_alone() {
        let mut config = Config::default();
        config.startup.show = StartupShow::Nothing;
        config.display.on_exit = crate::config::OnExit::Clear;
        let manager = Manager::new(Registry::new(), &config, Metrics::default()).unwrap();
        let (log, _) = attach(&manager, "a");
        manager.close();
        assert!(!log.calls().contains(&Call::Clear), "{:?}", log.calls());
        // Shutting down applies `on_exit`.
        let (log, _) = attach(&manager, "a");
        manager.shutdown();
        assert!(log.calls().contains(&Call::Clear), "{:?}", log.calls());
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
    fn a_stream_survives_a_reconnect() {
        let manager = manager(StartupShow::Nothing);
        let (_, unplug) = attach(&manager, "a");
        let ticket = manager.begin_stream("default").unwrap();
        unplug.unplug();
        // A frame of the stream finds the display gone.
        let device = manager.device("fake-0001").unwrap();
        let _ = device.submit(Frame::blank(1920, 462));
        wait_until(|| !device.presenter.is_running());
        manager.scan();
        attach(&manager, "a2");
        assert_eq!(manager.displays()[0].content, "stream");
        assert!(ticket.is_current());
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
