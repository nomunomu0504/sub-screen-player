use std::time::Duration;

use crate::{Error, Result};

/// How a landscape frame must be turned before it is encoded for the panel.
///
/// Many small panels are scanned in portrait, so a 1920x462 landscape frame is sent as a
/// 462x1920 image turned clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Send the frame as it is.
    None,
    /// Turn the frame 90 degrees clockwise.
    Clockwise90,
    /// Turn the frame 180 degrees.
    Half,
    /// Turn the frame 90 degrees counter-clockwise.
    CounterClockwise90,
}

/// How images are encoded for the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageFormat {
    /// Baseline JPEG with 4:2:0 chroma subsampling.
    Jpeg,
}

/// The visible panel as callers see it, and how frames are put on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelSpec {
    /// Width of a frame as callers draw it, in pixels.
    pub width: u32,
    /// Height of a frame as callers draw it, in pixels.
    pub height: u32,
    /// Turn applied before encoding.
    pub rotation: Rotation,
    /// Encoding the device expects.
    pub format: ImageFormat,
}

impl PanelSpec {
    /// Size of the encoded image the device receives (after the rotation).
    pub fn encoded_size(&self) -> (u32, u32) {
        match self.rotation {
            Rotation::None | Rotation::Half => (self.width, self.height),
            Rotation::Clockwise90 | Rotation::CounterClockwise90 => (self.height, self.width),
        }
    }
}

/// What a display can do. Callers check this before offering a feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// Frames can be shown without being stored on the device.
    pub live_frames: bool,
    /// Images can be stored so they survive power cycles.
    pub saved_frames: bool,
    /// The backlight can be dimmed.
    pub brightness: bool,
    /// The screen can be switched off and on.
    pub power: bool,
    /// The screen can be blanked.
    pub clear: bool,
    /// Highest frame rate known to work.
    pub max_fps: u32,
    /// How often [`Display::keep_alive`] must be called while a session is open, if at all.
    pub keep_alive_interval: Option<Duration>,
    /// Largest encoded image the device accepts.
    pub max_image_bytes: usize,
    /// [`Display::show`] takes images that cover only a part of the panel, drawn at their
    /// position over what is on screen.
    pub partial_images: bool,
}

/// Identity and properties of one connected display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Id of the driver that handles it, e.g. `"d92"`.
    pub driver: &'static str,
    /// Human-readable model name.
    pub model: String,
    /// Serial number as reported over USB (may be empty).
    pub serial: String,
    /// Firmware version, if the device reports one.
    pub firmware: Option<String>,
    /// Panel geometry.
    pub panel: PanelSpec,
    /// Supported operations.
    pub capabilities: Capabilities,
}

impl DisplayInfo {
    /// Stable id used by the daemon's API, e.g. `"d92-470B03781D1F"`.
    pub fn id(&self) -> String {
        if self.serial.is_empty() {
            self.driver.to_string()
        } else {
            format!("{}-{}", self.driver, self.serial)
        }
    }
}

/// An image ready for the wire: encoded and already in the panel's orientation.
///
/// Usually it covers the whole panel. On displays with [`Capabilities::partial_images`] it can
/// also be a part of the panel, placed at `x`, `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedImage {
    /// Encoded bytes.
    pub data: Vec<u8>,
    /// Width of the encoded image.
    pub width: u32,
    /// Height of the encoded image.
    pub height: u32,
    /// Encoding of `data`.
    pub format: ImageFormat,
    /// Left edge on the panel, in the panel's orientation (0 for a whole image).
    pub x: u32,
    /// Top edge on the panel, in the panel's orientation (0 for a whole image).
    pub y: u32,
}

impl EncodedImage {
    /// Whether the image covers the whole of `panel`.
    pub fn is_whole(&self, panel: &PanelSpec) -> bool {
        (self.x, self.y, self.width, self.height)
            == (0, 0, panel.encoded_size().0, panel.encoded_size().1)
    }
}

/// One connected screen. Implemented by each driver in `crates/drivers/<model>`.
///
/// Methods are called from a single thread at a time (the [`crate::Presenter`] owns the
/// display), so implementations do not need internal locking. Operations a model does not have
/// keep the default implementation, which returns [`Error::Unsupported`].
pub trait Display: Send {
    /// Identity and capabilities. Must not change while the display is open.
    fn info(&self) -> &DisplayInfo;

    /// Shows an image now. Must not write to the device's persistent memory.
    fn show(&mut self, image: &EncodedImage) -> Result<()>;

    /// Shows an image and stores it so it survives power cycles.
    fn save(&mut self, _image: &EncodedImage) -> Result<()> {
        Err(Error::Unsupported("saving images"))
    }

    /// Sets the backlight to `percent` (0..=100).
    fn set_brightness(&mut self, _percent: u8) -> Result<()> {
        Err(Error::Unsupported("brightness"))
    }

    /// Switches the screen on.
    fn wake(&mut self) -> Result<()> {
        Err(Error::Unsupported("power control"))
    }

    /// Switches the screen off.
    fn sleep(&mut self) -> Result<()> {
        Err(Error::Unsupported("power control"))
    }

    /// Blanks the screen.
    fn clear(&mut self) -> Result<()> {
        Err(Error::Unsupported("clearing"))
    }

    /// Tells the device the host is still there. Called every
    /// [`Capabilities::keep_alive_interval`] while the display is open.
    fn keep_alive(&mut self) -> Result<()> {
        Ok(())
    }
}
