//! Wire format of the D92. Pure functions that build output reports; no I/O.
//!
//! Every report is 1024 bytes. Commands start with `CRT\0\0` and a command word; image data
//! follows in raw reports without a prefix. The device never answers. The device's behavior is
//! described in `docs/devices/d92.md`.
//!
//! Never send `CRT\0\0SCREEN\0` (an "extended screen" mode): the device then
//! ignores all images until it is physically unplugged.

/// USB vendor id.
pub const VENDOR_ID: u16 = 0x2100;
/// USB product id.
pub const PRODUCT_ID: u16 = 0x0006;
/// HID usage page of the command interface.
pub const USAGE_PAGE: u16 = 0xFFA0;

/// Panel width as callers draw it (landscape).
pub const WIDTH: u32 = 1920;
/// Panel height as callers draw it (landscape).
pub const HEIGHT: u32 = 462;

/// Size of every output report, without the report ID.
pub const REPORT_LEN: usize = 1024;

/// Largest JPEG the driver sends.
pub const MAX_JPEG_LEN: usize = 512 << 10;

/// Length of the `DRA` header that precedes a live frame in the same report.
const LIVE_HEADER_LEN: usize = 32;

/// Byte 12 of every `DRA` header.
const LIVE_FLAG: u8 = 0xB1;

/// Byte 12 of a `LOG` header: what a stored image is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StoreMode {
    /// Becomes the boot screen. Overwrites the factory logo; not used by this driver.
    BootLogo = 0x01,
    /// Shown now and kept across power cycles.
    Saved = 0x02,
}

/// One command report: `CRT\0\0`, `word`, then `args`, zero-padded.
pub fn command(word: &[u8], args: &[u8]) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    let mut n = 0;
    for part in [b"CRT\0\0".as_slice(), word, args] {
        report[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    report
}

/// Switches the screen on (`DIS`).
pub fn wake() -> [u8; REPORT_LEN] {
    command(b"DIS", &[])
}

/// Switches the screen off (`HAN`).
pub fn sleep() -> [u8; REPORT_LEN] {
    command(b"HAN", &[])
}

/// Sets the backlight in percent (`LIG`).
pub fn brightness(percent: u8) -> [u8; REPORT_LEN] {
    command(b"LIG", &[0, 0, percent.min(100)])
}

/// Keeps the session open (`CONNECT`). Without it the device restarts about 8 s after the last
/// keep-alive.
pub fn keep_alive() -> [u8; REPORT_LEN] {
    command(b"CONNECT", &[])
}

/// Blanks the screen and ends the session (`CLE` `DC`).
pub fn clear() -> [u8; REPORT_LEN] {
    command(b"CLE", b"\0\0DC")
}

/// A live frame (`DRA`): shown at once, not stored. Returns whole reports back to back.
///
/// Layout of the first report: `CRT\0\0DRA`, `[8..12]` header + JPEG length (big-endian),
/// `[12]` = 0xB1, zeros up to byte 32, then the JPEG, which continues in the next reports.
pub fn live_frame(jpeg: &[u8]) -> Vec<u8> {
    let total = LIVE_HEADER_LEN + jpeg.len();
    let mut out = Vec::with_capacity(total.next_multiple_of(REPORT_LEN));
    out.extend_from_slice(b"CRT\0\0DRA");
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.push(LIVE_FLAG);
    out.resize(LIVE_HEADER_LEN, 0);
    out.extend_from_slice(jpeg);
    pad_to_reports(&mut out);
    out
}

/// A live image of a part of the panel (`DRA` with a position): drawn at `x`, `y` over what is on
/// screen. `width` and `height` are the JPEG's size. Coordinates are on the 462x1920 panel.
///
/// Same layout as [`live_frame`], plus `[13..21]` = width, height, x, y (u16 big-endian).
pub fn live_region(jpeg: &[u8], x: u16, y: u16, width: u16, height: u16) -> Vec<u8> {
    let mut out = live_frame(jpeg);
    for (i, value) in [width, height, x, y].into_iter().enumerate() {
        out[13 + 2 * i..15 + 2 * i].copy_from_slice(&value.to_be_bytes());
    }
    out
}

/// Shortest time between the starts of two `DRA`s. The device needs about 4 ms per image and
/// drops images that come faster (this only matters for small ones; larger ones take longer
/// to send anyway).
pub const DRA_SPACING: std::time::Duration = std::time::Duration::from_millis(5);

/// A stored image (`LOG`): header report, JPEG in raw reports, then `STP`.
/// The device needs about 1.5 s to store it.
pub fn stored_image(jpeg: &[u8], mode: StoreMode) -> Vec<u8> {
    let mut args = (jpeg.len() as u32).to_be_bytes().to_vec();
    args.push(mode as u8);
    let mut out = command(b"LOG", &args).to_vec();
    out.extend_from_slice(jpeg);
    pad_to_reports(&mut out);
    out.extend_from_slice(&command(b"STP", &[]));
    out
}

fn pad_to_reports(buf: &mut Vec<u8>) {
    buf.resize(buf.len().next_multiple_of(REPORT_LEN), 0);
}

/// Extracts the firmware version from the input report the device returns on `GET_REPORT`,
/// e.g. `V25.upHere_gamingD92.02.014`.
pub fn parse_version(report: &[u8]) -> Option<String> {
    let end = report.iter().position(|&b| b == 0).unwrap_or(report.len());
    let text = String::from_utf8_lossy(&report[..end]);
    let text = text.trim();
    (!text.is_empty() && text.chars().all(|c| c.is_ascii_graphic())).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_have_the_expected_bytes() {
        assert_eq!(&wake()[..8], b"CRT\0\0DIS");
        assert_eq!(&sleep()[..8], b"CRT\0\0HAN");
        assert_eq!(&brightness(50)[..11], b"CRT\0\0LIG\0\0\x32");
        assert_eq!(&keep_alive()[..12], b"CRT\0\0CONNECT");
        assert_eq!(&clear()[..12], b"CRT\0\0CLE\0\0DC");
        assert!(clear()[12..].iter().all(|&b| b == 0));
    }

    #[test]
    fn brightness_is_capped() {
        assert_eq!(brightness(250)[10], 100);
    }

    /// Header of a known-good 14547-byte frame.
    #[test]
    fn live_frame_header_is_exact() {
        let jpeg = vec![0xAB; 14547];
        let out = live_frame(&jpeg);
        assert_eq!(
            &out[..13],
            &[
                0x43, 0x52, 0x54, 0x00, 0x00, 0x44, 0x52, 0x41, 0x00, 0x00, 0x38, 0xf3, 0xb1
            ]
        );
        assert!(out[13..32].iter().all(|&b| b == 0));
        assert_eq!(&out[32..32 + jpeg.len()], jpeg.as_slice());
        // 32 + 14547 bytes need 15 reports; the rest is zero.
        assert_eq!(out.len(), 15 * REPORT_LEN);
        assert!(out[32 + jpeg.len()..].iter().all(|&b| b == 0));
    }

    #[test]
    fn live_region_carries_its_place() {
        let jpeg = vec![0xAB; 100];
        let out = live_region(&jpeg, 400, 96, 16, 32);
        assert_eq!(&out[..13], &live_frame(&jpeg)[..13]);
        assert_eq!(&out[13..21], &[0, 16, 0, 32, 0x01, 0x90, 0, 96]);
        assert!(out[21..32].iter().all(|&b| b == 0));
        assert_eq!(&out[32..132], jpeg.as_slice());
    }

    #[test]
    fn live_frame_that_fills_reports_exactly_gets_no_extra_report() {
        assert_eq!(live_frame(&[1; REPORT_LEN - 32]).len(), REPORT_LEN);
    }

    #[test]
    fn stored_image_is_header_chunks_and_commit() {
        let jpeg = vec![0xCD; 2000];
        let out = stored_image(&jpeg, StoreMode::Saved);
        let reports: Vec<&[u8]> = out.chunks(REPORT_LEN).collect();
        assert_eq!(reports.len(), 4);
        assert_eq!(&reports[0][..13], b"CRT\0\0LOG\0\0\x07\xd0\x02");
        assert_eq!(reports[1], &jpeg[..REPORT_LEN]);
        assert_eq!(&reports[2][..976], &jpeg[REPORT_LEN..]);
        assert!(reports[2][976..].iter().all(|&b| b == 0));
        assert_eq!(&reports[3][..8], b"CRT\0\0STP");
    }

    #[test]
    fn parses_version_report() {
        let mut report = b"V25.upHere_gamingD92.02.014".to_vec();
        report.resize(512, 0);
        assert_eq!(
            parse_version(&report).as_deref(),
            Some("V25.upHere_gamingD92.02.014")
        );
        assert_eq!(parse_version(&[0; 16]), None);
        assert_eq!(parse_version(&[0xFF, 0x01, 0]), None);
    }
}
