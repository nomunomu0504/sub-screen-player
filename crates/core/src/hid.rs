//! USB HID access through `hidapi`, shared by all HID-based drivers.

use std::sync::{Mutex, MutexGuard, OnceLock};

use hidapi::{HidApi, HidDevice};

use crate::{Candidate, Error, Result, Transport};

fn api() -> Result<MutexGuard<'static, HidApi>> {
    static API: OnceLock<Mutex<HidApi>> = OnceLock::new();
    let api = match API.get() {
        Some(api) => api,
        None => {
            let created = create()?;
            API.get_or_init(|| Mutex::new(created))
        }
    };
    // A panic while holding the lock leaves the device list intact, so keep using it.
    Ok(api.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// Creates the `HidApi`. On macOS, hidapi ties its device manager to the run loop of the thread
/// that creates it, and the next enumeration crashes once that thread has ended (as the daemon's
/// scanner thread does when the config is reloaded), so it is created on a thread that never
/// ends.
#[cfg(target_os = "macos")]
fn create() -> Result<HidApi> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("ssp-hid".into())
        .spawn(move || {
            let _ = sender.send(HidApi::new());
            loop {
                std::thread::park();
            }
        })
        .map_err(|e| Error::Transport(format!("cannot start the HID thread: {e}")))?;
    receiver
        .recv()
        .map_err(|_| Error::Transport("the HID thread ended".into()))?
        .map_err(to_error)
}

#[cfg(not(target_os = "macos"))]
fn create() -> Result<HidApi> {
    HidApi::new().map_err(to_error)
}

fn to_error(err: hidapi::HidError) -> Error {
    Error::Transport(err.to_string())
}

/// Lists the HID interfaces currently connected.
pub fn enumerate() -> Result<Vec<Candidate>> {
    let mut api = api()?;
    api.refresh_devices().map_err(to_error)?;
    let mut out: Vec<Candidate> = Vec::new();
    for info in api.device_list() {
        if out.iter().any(|c| c.path.as_c_str() == info.path()) {
            continue;
        }
        out.push(Candidate {
            path: info.path().to_owned(),
            vendor_id: info.vendor_id(),
            product_id: info.product_id(),
            usage_page: info.usage_page(),
            serial: info.serial_number().unwrap_or_default().to_string(),
            product: info.product_string().unwrap_or_default().to_string(),
        });
    }
    Ok(out)
}

/// Opens a HID interface found by [`enumerate`].
pub fn open(candidate: &Candidate) -> Result<HidTransport> {
    let device = api()?.open_path(&candidate.path).map_err(to_error)?;
    Ok(HidTransport {
        device,
        buf: Vec::new(),
    })
}

/// A [`Transport`] over one HID interface. Reports are sent with report ID 0.
pub struct HidTransport {
    device: HidDevice,
    buf: Vec<u8>,
}

impl Transport for HidTransport {
    fn write_report(&mut self, report: &[u8]) -> Result<()> {
        self.buf.clear();
        self.buf.push(0);
        self.buf.extend_from_slice(report);
        let written = self.device.write(&self.buf).map_err(to_error)?;
        // Some backends count the report ID, some do not.
        if written < report.len() {
            return Err(Error::Transport(format!(
                "short write: {written} of {} bytes",
                report.len()
            )));
        }
        Ok(())
    }

    fn get_input_report(&mut self, report_id: u8, buf: &mut [u8]) -> Result<usize> {
        let mut data = vec![0u8; buf.len() + 1];
        data[0] = report_id;
        let n = self.device.get_input_report(&mut data).map_err(to_error)?;
        let n = n.saturating_sub(1).min(buf.len());
        buf[..n].copy_from_slice(&data[1..=n]);
        Ok(n)
    }
}
