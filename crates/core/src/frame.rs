use std::str::FromStr;

use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};

use crate::{EncodedImage, Error, ImageFormat, PanelSpec, Result, Rotation};

/// How an image whose aspect ratio differs from the panel is fitted onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// Scale to fit inside the panel and fill the rest with black. Nothing is cut off.
    #[default]
    Contain,
    /// Scale to fill the panel and cut off what sticks out.
    Cover,
    /// Scale each axis independently. The image may look squashed.
    Stretch,
}

impl FromStr for Fit {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "contain" => Ok(Self::Contain),
            "cover" => Ok(Self::Cover),
            "stretch" => Ok(Self::Stretch),
            _ => Err(Error::InvalidArgument(format!(
                "unknown fit {s:?} (expected contain, cover or stretch)"
            ))),
        }
    }
}

/// One picture for a display, in landscape orientation and at the panel's size.
///
/// Renderers draw into a `Frame`; the [`Encoder`] turns it into what the device expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    image: RgbImage,
}

impl Frame {
    /// Wraps an image that already has the panel's size.
    pub fn new(image: RgbImage) -> Self {
        Self { image }
    }

    /// A black frame.
    pub fn blank(width: u32, height: u32) -> Self {
        Self::new(RgbImage::new(width, height))
    }

    /// Scales any image to `width` x `height`. Transparent areas become black.
    pub fn fit(image: &DynamicImage, width: u32, height: u32, fit: Fit) -> Self {
        let resized = if image.dimensions() == (width, height) {
            None
        } else {
            Some(match fit {
                Fit::Contain => image.resize(width, height, FilterType::Triangle),
                Fit::Cover => image.resize_to_fill(width, height, FilterType::Triangle),
                Fit::Stretch => image.resize_exact(width, height, FilterType::Triangle),
            })
        };
        let image = resized.as_ref().unwrap_or(image);
        let rgb = flatten(image);
        if rgb.dimensions() == (width, height) {
            return Self::new(rgb);
        }
        // Contain left a smaller image: center it on black.
        let mut canvas = RgbImage::new(width, height);
        let x = (width - rgb.width()) / 2;
        let y = (height - rgb.height()) / 2;
        image::imageops::replace(&mut canvas, &rgb, x.into(), y.into());
        Self::new(canvas)
    }

    /// Decodes an image file (PNG, JPEG, GIF or WebP) and fits it to `width` x `height`.
    pub fn decode(bytes: &[u8], width: u32, height: u32, fit: Fit) -> Result<Self> {
        let image = image::load_from_memory(bytes).map_err(|e| Error::Image(e.to_string()))?;
        Ok(Self::fit(&image, width, height, fit))
    }

    /// Builds a frame from tightly packed 8-bit RGB pixels.
    pub fn from_rgb(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self> {
        RgbImage::from_raw(width, height, pixels)
            .map(Self::new)
            .ok_or_else(|| Error::InvalidArgument(format!("expected {width}x{height}x3 bytes")))
    }

    /// Builds a frame from tightly packed 8-bit RGBA pixels. Alpha is blended onto black.
    pub fn from_rgba(width: u32, height: u32, pixels: &[u8]) -> Result<Self> {
        if pixels.len() != width as usize * height as usize * 4 {
            return Err(Error::InvalidArgument(format!(
                "expected {width}x{height}x4 bytes"
            )));
        }
        let blend = |c: u8, a: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        let rgb = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| [blend(r, a), blend(g, a), blend(b, a)])
            .collect();
        Self::from_rgb(width, height, rgb)
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.image.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.image.height()
    }

    /// The pixels.
    pub fn image(&self) -> &RgbImage {
        &self.image
    }

    /// The pixels, for drawing.
    pub fn image_mut(&mut self) -> &mut RgbImage {
        &mut self.image
    }

    /// Unwraps the pixels.
    pub fn into_image(self) -> RgbImage {
        self.image
    }
}

/// Converts to RGB, blending transparent pixels onto black.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    let (w, h) = rgba.dimensions();
    let frame = Frame::from_rgba(w, h, rgba.as_raw()).expect("buffer size matches dimensions");
    frame.into_image()
}

/// Turns [`Frame`]s into [`EncodedImage`]s for a panel. Keeps buffers between calls.
#[derive(Debug)]
pub struct Encoder {
    quality: u8,
    rotated: Vec<u8>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new(Self::DEFAULT_QUALITY)
    }
}

impl Encoder {
    /// JPEG quality used unless configured otherwise.
    pub const DEFAULT_QUALITY: u8 = 85;

    /// Lowest quality tried when an image is over the device's size limit.
    const MIN_QUALITY: u8 = 25;

    /// Creates an encoder. `quality` is the JPEG quality (1..=100).
    pub fn new(quality: u8) -> Self {
        Self {
            quality: quality.clamp(1, 100),
            rotated: Vec::new(),
        }
    }

    /// Rotates and encodes `frame` for `panel`. If the result is larger than `max_bytes`,
    /// the quality is lowered until it fits.
    pub fn encode(
        &mut self,
        frame: &Frame,
        panel: &PanelSpec,
        max_bytes: usize,
    ) -> Result<EncodedImage> {
        if (frame.width(), frame.height()) != (panel.width, panel.height) {
            return Err(Error::InvalidArgument(format!(
                "frame is {}x{}, panel is {}x{}",
                frame.width(),
                frame.height(),
                panel.width,
                panel.height
            )));
        }
        let (width, height) = panel.encoded_size();
        let pixels = match panel.rotation {
            Rotation::None => frame.image().as_raw(),
            rotation => {
                rotate(frame.image(), rotation, &mut self.rotated);
                &self.rotated
            }
        };
        let mut quality = self.quality;
        loop {
            let data = match panel.format {
                ImageFormat::Jpeg => encode_jpeg(pixels, width, height, quality)?,
            };
            if data.len() <= max_bytes {
                return Ok(EncodedImage {
                    data,
                    width,
                    height,
                    format: panel.format,
                });
            }
            if quality <= Self::MIN_QUALITY {
                return Err(Error::Image(format!(
                    "encoded image is {} bytes, the display accepts at most {max_bytes}",
                    data.len()
                )));
            }
            quality = quality.saturating_sub(10).max(Self::MIN_QUALITY);
        }
    }
}

fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    let (w, h) = match (u16::try_from(width), u16::try_from(height)) {
        (Ok(w), Ok(h)) => (w, h),
        _ => {
            return Err(Error::Image(format!(
                "{width}x{height} is too large for JPEG"
            )));
        }
    };
    let mut out = Vec::with_capacity(rgb.len() / 8);
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    // Baseline 4:2:0 is what small display firmwares decode reliably.
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    encoder
        .encode(rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| Error::Image(e.to_string()))?;
    Ok(out)
}

/// Writes `src` turned by `rotation` into `out` as packed RGB.
fn rotate(src: &RgbImage, rotation: Rotation, out: &mut Vec<u8>) {
    let (w, h) = (src.width() as usize, src.height() as usize);
    let s = src.as_raw();
    out.clear();
    out.reserve(w * h * 3);
    let mut push = |x: usize, y: usize| {
        let i = (y * w + x) * 3;
        out.extend_from_slice(&s[i..i + 3]);
    };
    match rotation {
        Rotation::None => (0..h).for_each(|y| (0..w).for_each(|x| push(x, y))),
        // Output is h wide and w tall; its row `oy` is source column `oy`, read bottom-up.
        Rotation::Clockwise90 => (0..w).for_each(|oy| (0..h).rev().for_each(|y| push(oy, y))),
        Rotation::CounterClockwise90 => {
            (0..w).rev().for_each(|x| (0..h).for_each(|y| push(x, y)));
        }
        Rotation::Half => (0..h)
            .rev()
            .for_each(|y| (0..w).rev().for_each(|x| push(x, y))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(rotation: Rotation) -> PanelSpec {
        PanelSpec {
            width: 3,
            height: 2,
            rotation,
            format: ImageFormat::Jpeg,
        }
    }

    /// 3x2 image whose red channel numbers the pixels:
    /// ```text
    /// 0 1 2
    /// 3 4 5
    /// ```
    fn numbered() -> RgbImage {
        RgbImage::from_fn(3, 2, |x, y| image::Rgb([(y * 3 + x) as u8, 0, 0]))
    }

    fn reds(rotation: Rotation) -> Vec<u8> {
        let mut out = Vec::new();
        rotate(&numbered(), rotation, &mut out);
        out.chunks(3).map(|p| p[0]).collect()
    }

    #[test]
    fn rotates_clockwise() {
        // 3 0
        // 4 1
        // 5 2
        assert_eq!(reds(Rotation::Clockwise90), [3, 0, 4, 1, 5, 2]);
        assert_eq!(panel(Rotation::Clockwise90).encoded_size(), (2, 3));
    }

    #[test]
    fn rotates_counter_clockwise_and_half() {
        assert_eq!(reds(Rotation::CounterClockwise90), [2, 5, 1, 4, 0, 3]);
        assert_eq!(reds(Rotation::Half), [5, 4, 3, 2, 1, 0]);
        assert_eq!(reds(Rotation::None), [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn encodes_baseline_jpeg_in_panel_orientation() {
        let spec = PanelSpec {
            width: 64,
            height: 16,
            rotation: Rotation::Clockwise90,
            format: ImageFormat::Jpeg,
        };
        let image = Encoder::default()
            .encode(&Frame::blank(64, 16), &spec, usize::MAX)
            .unwrap();
        assert_eq!((image.width, image.height), (16, 64));
        assert_eq!(&image.data[..2], &[0xFF, 0xD8]);
        let decoded = image::load_from_memory(&image.data).unwrap();
        assert_eq!(decoded.dimensions(), (16, 64));
    }

    #[test]
    fn rejects_frames_of_the_wrong_size() {
        let err = Encoder::default()
            .encode(&Frame::blank(10, 10), &panel(Rotation::None), usize::MAX)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidArgument(_)));
    }

    #[test]
    fn lowers_quality_to_fit_the_size_limit() {
        let noisy = RgbImage::from_fn(64, 64, |x, y| {
            let v = (x * 7919 + y * 104_729) as u8;
            image::Rgb([v, v.wrapping_mul(3), v.wrapping_mul(7)])
        });
        let spec = PanelSpec {
            width: 64,
            height: 64,
            rotation: Rotation::None,
            format: ImageFormat::Jpeg,
        };
        let mut encoder = Encoder::new(100);
        let best = encoder
            .encode(&Frame::new(noisy.clone()), &spec, usize::MAX)
            .unwrap();
        let limited = encoder
            .encode(&Frame::new(noisy), &spec, best.data.len() - 1)
            .unwrap();
        assert!(limited.data.len() < best.data.len());
    }

    #[test]
    fn contain_letterboxes_on_black() {
        let white = DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 10, image::Rgb([255; 3])));
        let frame = Frame::fit(&white, 40, 10, Fit::Contain);
        assert_eq!(frame.image().get_pixel(0, 5).0, [0, 0, 0]);
        assert_eq!(frame.image().get_pixel(20, 5).0, [255, 255, 255]);
        let frame = Frame::fit(&white, 40, 10, Fit::Cover);
        assert_eq!(frame.image().get_pixel(0, 5).0, [255, 255, 255]);
    }

    #[test]
    fn blends_alpha_onto_black() {
        let frame = Frame::from_rgba(1, 1, &[200, 100, 50, 128]).unwrap();
        assert_eq!(frame.image().get_pixel(0, 0).0, [100, 50, 25]);
    }
}
