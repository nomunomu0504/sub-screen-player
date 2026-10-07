//! Test doubles for drivers and for code that uses displays. No hardware needed.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::{
    Capabilities, Display, DisplayInfo, EncodedImage, Error, ImageFormat, PanelSpec, Result,
    Rotation, Transport,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A [`Transport`] that records every report it is given.
///
/// Clones share the same record, so a test can keep one clone and hand the other to a driver.
#[derive(Debug, Clone, Default)]
pub struct RecordingTransport {
    inner: Arc<Mutex<Recording>>,
}

#[derive(Debug, Default)]
struct Recording {
    reports: Vec<Vec<u8>>,
    input_report: Option<Vec<u8>>,
    fail_writes: bool,
}

impl RecordingTransport {
    /// Creates an empty recording.
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes [`Transport::get_input_report`] return `data`.
    pub fn with_input_report(self, data: &[u8]) -> Self {
        lock(&self.inner).input_report = Some(data.to_vec());
        self
    }

    /// Makes every following write fail, as if the device was unplugged.
    pub fn fail_writes(&self) {
        lock(&self.inner).fail_writes = true;
    }

    /// Returns and forgets the reports written so far.
    pub fn take(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut lock(&self.inner).reports)
    }
}

impl Transport for RecordingTransport {
    fn write_report(&mut self, report: &[u8]) -> Result<()> {
        let mut rec = lock(&self.inner);
        if rec.fail_writes {
            return Err(Error::Transport("write failed (test)".into()));
        }
        rec.reports.push(report.to_vec());
        Ok(())
    }

    fn get_input_report(&mut self, _report_id: u8, buf: &mut [u8]) -> Result<usize> {
        let rec = lock(&self.inner);
        let data = rec
            .input_report
            .as_deref()
            .ok_or(Error::Unsupported("input reports"))?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
}

/// A call received by a [`FakeDisplay`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// [`Display::show`] with the encoded bytes.
    Show(Vec<u8>),
    /// [`Display::save`] with the encoded bytes.
    Save(Vec<u8>),
    /// [`Display::set_brightness`].
    Brightness(u8),
    /// [`Display::wake`].
    Wake,
    /// [`Display::sleep`].
    Sleep,
    /// [`Display::clear`].
    Clear,
    /// [`Display::keep_alive`].
    KeepAlive,
}

/// Shared list of the calls a [`FakeDisplay`] received.
#[derive(Debug, Clone, Default)]
pub struct CallLog(Arc<Mutex<Vec<Call>>>);

impl CallLog {
    /// The calls so far.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.0).clone()
    }

    /// Returns and forgets the calls so far.
    pub fn take(&self) -> Vec<Call> {
        std::mem::take(&mut lock(&self.0))
    }
}

/// A [`Display`] that supports everything and only records what it is asked to do.
#[derive(Debug)]
pub struct FakeDisplay {
    info: DisplayInfo,
    log: CallLog,
    show_delay: Duration,
    disconnected: Arc<Mutex<bool>>,
}

impl FakeDisplay {
    /// A 1920x462 display sent as rotated JPEGs, like the D92.
    pub fn new() -> (Self, CallLog) {
        Self::with_panel(PanelSpec {
            width: 1920,
            height: 462,
            rotation: Rotation::Clockwise90,
            format: ImageFormat::Jpeg,
        })
    }

    /// A display with the given panel.
    pub fn with_panel(panel: PanelSpec) -> (Self, CallLog) {
        let log = CallLog::default();
        let info = DisplayInfo {
            driver: "fake",
            model: "Fake display".into(),
            serial: "0001".into(),
            firmware: None,
            panel,
            capabilities: Capabilities {
                live_frames: true,
                saved_frames: true,
                brightness: true,
                power: true,
                clear: true,
                max_fps: 60,
                keep_alive_interval: None,
                max_image_bytes: 4 << 20,
            },
        };
        let display = Self {
            info,
            log: log.clone(),
            show_delay: Duration::ZERO,
            disconnected: Arc::default(),
        };
        (display, log)
    }

    /// Makes [`Display::show`] take `delay`, like a slow USB link.
    pub fn show_delay(mut self, delay: Duration) -> Self {
        self.show_delay = delay;
        self
    }

    /// Sets how often the display wants keep-alives.
    pub fn keep_alive_interval(mut self, interval: Duration) -> Self {
        self.info.capabilities.keep_alive_interval = Some(interval);
        self
    }

    /// Returns a handle that makes every following call fail as if the device was unplugged.
    pub fn unplug_handle(&self) -> Unplug {
        Unplug(self.disconnected.clone())
    }

    fn record(&self, call: Call) -> Result<()> {
        if *lock(&self.disconnected) {
            return Err(Error::Disconnected);
        }
        lock(&self.log.0).push(call);
        Ok(())
    }
}

/// See [`FakeDisplay::unplug_handle`].
#[derive(Debug, Clone)]
pub struct Unplug(Arc<Mutex<bool>>);

impl Unplug {
    /// Simulates unplugging the device.
    pub fn unplug(&self) {
        *lock(&self.0) = true;
    }
}

impl Display for FakeDisplay {
    fn info(&self) -> &DisplayInfo {
        &self.info
    }

    fn show(&mut self, image: &EncodedImage) -> Result<()> {
        if !self.show_delay.is_zero() {
            std::thread::sleep(self.show_delay);
        }
        self.record(Call::Show(image.data.clone()))
    }

    fn save(&mut self, image: &EncodedImage) -> Result<()> {
        self.record(Call::Save(image.data.clone()))
    }

    fn set_brightness(&mut self, percent: u8) -> Result<()> {
        self.record(Call::Brightness(percent))
    }

    fn wake(&mut self) -> Result<()> {
        self.record(Call::Wake)
    }

    fn sleep(&mut self) -> Result<()> {
        self.record(Call::Sleep)
    }

    fn clear(&mut self) -> Result<()> {
        self.record(Call::Clear)
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.record(Call::KeepAlive)
    }
}
