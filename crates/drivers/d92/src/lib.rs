//! Driver for the D92 9.2-inch 1920x462 USB display, sold as the upHere D92 and the
//! MiraBox D92 (USB `2100:0006`).
//!
//! - [`protocol`] builds the reports (pure, unit-tested against captures).
//! - [`D92`] implements [`ssp_core::Display`] on top of any [`Transport`].
//! - [`D92Driver`] finds the device and opens it over HID.
//!
//! Protocol notes: `docs/devices/d92.md`.
#![warn(missing_docs)]

pub mod protocol;

use std::time::{Duration, Instant};

use ssp_core::{
    Candidate, Capabilities, Display, DisplayInfo, Driver, EncodedImage, Error, ImageFormat,
    PanelSpec, Result, Rotation, Transport, UsbMatch, hid,
};

use protocol::{REPORT_LEN, StoreMode};

/// How long the device needs to store an image; frames sent earlier get lost.
const STORE_TIME: Duration = Duration::from_millis(1500);

/// Finds D92 displays and opens them over HID.
#[derive(Debug, Clone, Copy, Default)]
pub struct D92Driver;

impl Driver for D92Driver {
    fn id(&self) -> &'static str {
        "d92"
    }

    fn name(&self) -> &'static str {
        "upHere / MiraBox D92"
    }

    fn usb_matches(&self) -> &'static [UsbMatch] {
        &[UsbMatch {
            vendor_id: protocol::VENDOR_ID,
            product_id: protocol::PRODUCT_ID,
            usage_page: Some(protocol::USAGE_PAGE),
        }]
    }

    fn open(&self, candidate: &Candidate) -> Result<Box<dyn Display>> {
        let transport = hid::open(candidate)?;
        Ok(Box::new(D92::open(Box::new(transport), &candidate.serial)?))
    }
}

/// An open D92.
pub struct D92 {
    transport: Box<dyn Transport>,
    info: DisplayInfo,
    /// Set by `clear`, which ends the device's session; the next frame wakes it first.
    session_ended: bool,
    /// Until when the device is busy storing an image.
    busy_until: Option<Instant>,
}

impl D92 {
    /// Takes over a device: reads its firmware version (where the platform allows it) and
    /// switches the screen on.
    pub fn open(mut transport: Box<dyn Transport>, serial: &str) -> Result<Self> {
        let mut report = [0u8; 512];
        let firmware = match transport.get_input_report(0, &mut report) {
            Ok(n) => protocol::parse_version(&report[..n]),
            Err(err) if err.is_fatal() => return Err(err),
            Err(_) => None,
        };
        let model = match &firmware {
            Some(v) if v.contains("upHere") => "upHere D92",
            _ => "D92",
        };
        let info = DisplayInfo {
            driver: "d92",
            model: model.to_string(),
            serial: serial.to_string(),
            firmware,
            panel: PanelSpec {
                width: protocol::WIDTH,
                height: protocol::HEIGHT,
                // The controller scans the panel in portrait.
                rotation: Rotation::Clockwise90,
                format: ImageFormat::Jpeg,
            },
            capabilities: Capabilities {
                live_frames: true,
                saved_frames: true,
                brightness: true,
                power: true,
                clear: true,
                max_fps: 60,
                keep_alive_interval: Some(Duration::from_secs(2)),
                max_image_bytes: protocol::MAX_JPEG_LEN,
            },
        };
        let mut d92 = Self {
            transport,
            info,
            session_ended: false,
            busy_until: None,
        };
        d92.send(&protocol::wake())?;
        Ok(d92)
    }

    fn send(&mut self, reports: &[u8]) -> Result<()> {
        debug_assert_eq!(reports.len() % REPORT_LEN, 0);
        reports
            .chunks(REPORT_LEN)
            .try_for_each(|report| self.transport.write_report(report))
    }

    fn check(&self, image: &EncodedImage) -> Result<()> {
        let (width, height) = self.info.panel.encoded_size();
        if image.format != ImageFormat::Jpeg
            || (image.width, image.height) != (width, height)
            || !image.data.starts_with(&[0xFF, 0xD8])
        {
            return Err(Error::InvalidArgument(format!(
                "the D92 takes {width}x{height} JPEGs"
            )));
        }
        if image.data.len() > protocol::MAX_JPEG_LEN {
            return Err(Error::InvalidArgument(format!(
                "JPEG is {} bytes, the D92 takes at most {}",
                image.data.len(),
                protocol::MAX_JPEG_LEN
            )));
        }
        Ok(())
    }

    /// Prepares for an image: waits for a running store and restarts an ended session.
    fn before_image(&mut self) -> Result<()> {
        if let Some(until) = self.busy_until.take() {
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
        }
        if self.session_ended {
            self.send(&protocol::wake())?;
            self.session_ended = false;
        }
        Ok(())
    }
}

impl Display for D92 {
    fn info(&self) -> &DisplayInfo {
        &self.info
    }

    fn show(&mut self, image: &EncodedImage) -> Result<()> {
        self.check(image)?;
        self.before_image()?;
        self.send(&protocol::live_frame(&image.data))
    }

    fn save(&mut self, image: &EncodedImage) -> Result<()> {
        self.check(image)?;
        self.before_image()?;
        self.send(&protocol::stored_image(&image.data, StoreMode::Saved))?;
        self.busy_until = Some(Instant::now() + STORE_TIME);
        Ok(())
    }

    fn set_brightness(&mut self, percent: u8) -> Result<()> {
        self.send(&protocol::brightness(percent))
    }

    fn wake(&mut self) -> Result<()> {
        self.session_ended = false;
        self.send(&protocol::wake())
    }

    fn sleep(&mut self) -> Result<()> {
        self.send(&protocol::sleep())
    }

    fn clear(&mut self) -> Result<()> {
        self.send(&protocol::clear())?;
        self.session_ended = true;
        Ok(())
    }

    fn keep_alive(&mut self) -> Result<()> {
        self.send(&protocol::keep_alive())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssp_core::testing::RecordingTransport;
    use ssp_core::{Encoder, Frame};

    fn open() -> (D92, RecordingTransport) {
        let transport =
            RecordingTransport::new().with_input_report(b"V25.upHere_gamingD92.02.014\0\0");
        let d92 = D92::open(Box::new(transport.clone()), "470B03781D1F").unwrap();
        (d92, transport)
    }

    fn jpeg(d92: &D92) -> EncodedImage {
        let frame = Frame::blank(protocol::WIDTH, protocol::HEIGHT);
        Encoder::default()
            .encode(&frame, &d92.info().panel, protocol::MAX_JPEG_LEN)
            .unwrap()
    }

    fn words(reports: &[Vec<u8>]) -> Vec<String> {
        reports
            .iter()
            .filter(|r| r.starts_with(b"CRT\0\0"))
            .map(|r| {
                r[5..]
                    .iter()
                    .take_while(|b| b.is_ascii_uppercase())
                    .map(|&b| b as char)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn open_reads_version_and_wakes() {
        let (d92, transport) = open();
        assert_eq!(
            d92.info().firmware.as_deref(),
            Some("V25.upHere_gamingD92.02.014")
        );
        assert_eq!(d92.info().model, "upHere D92");
        assert_eq!(d92.info().id(), "d92-470B03781D1F");
        assert_eq!(words(&transport.take()), ["DIS"]);
    }

    #[test]
    fn open_works_without_version_report() {
        let d92 = D92::open(Box::new(RecordingTransport::new()), "").unwrap();
        assert_eq!(d92.info().firmware, None);
        assert_eq!(d92.info().id(), "d92");
    }

    #[test]
    fn show_sends_one_live_frame() {
        let (mut d92, transport) = open();
        transport.take();
        let image = jpeg(&d92);
        d92.show(&image).unwrap();
        let reports = transport.take();
        assert_eq!(reports.len(), (32 + image.data.len()).div_ceil(REPORT_LEN));
        assert!(reports.iter().all(|r| r.len() == REPORT_LEN));
        assert_eq!(words(&reports), ["DRA"]);
        assert_eq!(&reports[0][32..34], &[0xFF, 0xD8]);
    }

    #[test]
    fn show_after_clear_wakes_first() {
        let (mut d92, transport) = open();
        d92.clear().unwrap();
        let image = jpeg(&d92);
        d92.show(&image).unwrap();
        d92.show(&image).unwrap();
        assert_eq!(
            words(&transport.take()),
            ["DIS", "CLE", "DIS", "DRA", "DRA"]
        );
    }

    #[test]
    fn save_stores_with_log_and_stp() {
        let (mut d92, transport) = open();
        transport.take();
        d92.save(&jpeg(&d92)).unwrap();
        let reports = transport.take();
        assert_eq!(words(&reports), ["LOG", "STP"]);
        assert_eq!(reports[0][12], StoreMode::Saved as u8);
    }

    #[test]
    fn rejects_images_in_the_wrong_orientation() {
        let (mut d92, _) = open();
        let mut image = jpeg(&d92);
        std::mem::swap(&mut image.width, &mut image.height);
        assert!(matches!(d92.show(&image), Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn write_errors_are_fatal() {
        let (mut d92, transport) = open();
        transport.fail_writes();
        assert!(d92.keep_alive().unwrap_err().is_fatal());
    }

    #[test]
    fn never_sends_screen() {
        let (mut d92, transport) = open();
        let image = jpeg(&d92);
        d92.show(&image).unwrap();
        d92.set_brightness(30).unwrap();
        d92.sleep().unwrap();
        d92.wake().unwrap();
        d92.keep_alive().unwrap();
        d92.clear().unwrap();
        assert!(
            !transport
                .take()
                .iter()
                .any(|r| r.starts_with(b"CRT\0\0SCREEN"))
        );
    }
}
