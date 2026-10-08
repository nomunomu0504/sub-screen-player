use std::io::Cursor;
use std::time::Duration;

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, Frames, ImageFormat as FileFormat};

use crate::{Error, Result};

/// Most frames an animation may have.
pub const MAX_FRAMES: usize = 1000;
/// Most memory the decoded frames (8-bit RGBA) may take.
pub const MAX_BYTES: usize = 256 << 20;
/// Delays shorter than this are played as [`DEFAULT_DELAY`], as browsers do: many GIFs say
/// 0 or 10 ms and mean "the default speed".
const MIN_DELAY: Duration = Duration::from_millis(20);
/// See [`MIN_DELAY`].
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// An animated image (GIF, APNG or animated WebP): its frames at their original size, each with
/// the time it stays on screen. Playback loops forever.
#[derive(Debug, Clone)]
pub struct Animation {
    frames: Vec<(DynamicImage, Duration)>,
    total: Duration,
}

impl Animation {
    /// Decodes an animated GIF, APNG or WebP.
    ///
    /// Returns `Ok(None)` for anything that is not an animation with at least two frames (still
    /// images, other formats), so callers can treat the bytes as a still image instead.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>> {
        let Ok(format) = image::guess_format(bytes) else {
            return Ok(None);
        };
        let frames = match format {
            FileFormat::Gif => collect(
                GifDecoder::new(Cursor::new(bytes))
                    .map_err(image)?
                    .into_frames(),
            )?,
            FileFormat::Png => {
                let decoder = PngDecoder::new(Cursor::new(bytes)).map_err(image)?;
                if !decoder.is_apng().map_err(image)? {
                    return Ok(None);
                }
                collect(decoder.apng().map_err(image)?.into_frames())?
            }
            FileFormat::WebP => {
                let decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(image)?;
                if !decoder.has_animation() {
                    return Ok(None);
                }
                collect(decoder.into_frames())?
            }
            _ => return Ok(None),
        };
        if frames.len() < 2 {
            return Ok(None);
        }
        let total = frames.iter().map(|(_, delay)| *delay).sum();
        Ok(Some(Self { frames, total }))
    }

    /// Number of frames (at least 2).
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Always `false`: an animation has at least two frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Frame `index` at its original size.
    ///
    /// # Panics
    ///
    /// If `index` is not below [`Animation::len`].
    pub fn frame(&self, index: usize) -> &DynamicImage {
        &self.frames[index].0
    }

    /// How long one pass through all frames takes.
    pub fn duration(&self) -> Duration {
        self.total
    }

    /// The frame to show `elapsed` after playback started, and how long until the next one.
    pub fn at(&self, elapsed: Duration) -> (usize, Duration) {
        let total = self.total.as_nanos().max(1);
        let mut position = Duration::from_nanos((elapsed.as_nanos() % total) as u64);
        for (index, (_, delay)) in self.frames.iter().enumerate() {
            if position < *delay {
                return (index, *delay - position);
            }
            position -= *delay;
        }
        // Only reachable through rounding: start over.
        (0, self.frames[0].1)
    }
}

fn collect(frames: Frames<'_>) -> Result<Vec<(DynamicImage, Duration)>> {
    let mut out = Vec::new();
    let mut bytes = 0;
    for frame in frames {
        let frame = frame.map_err(image)?;
        let delay = match Duration::from(frame.delay()) {
            short if short < MIN_DELAY => DEFAULT_DELAY,
            delay => delay,
        };
        let buffer = frame.into_buffer();
        bytes += buffer.as_raw().len();
        if out.len() == MAX_FRAMES || bytes > MAX_BYTES {
            return Err(Error::InvalidArgument(format!(
                "the animation is too long: at most {MAX_FRAMES} frames and {} MiB of pixels",
                MAX_BYTES >> 20
            )));
        }
        out.push((DynamicImage::ImageRgba8(buffer), delay));
    }
    Ok(out)
}

fn image(err: image::ImageError) -> Error {
    Error::Image(err.to_string())
}

#[cfg(test)]
mod tests {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame as ImageFrame, Rgba, RgbaImage};

    use super::*;

    /// A GIF with one frame per delay (in ms), each a different color.
    fn gif(delays_ms: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for (i, &ms) in delays_ms.iter().enumerate() {
                let shade = (i * 40 % 256) as u8;
                let image = RgbaImage::from_pixel(4, 2, Rgba([shade, 0, 255 - shade, 255]));
                let delay = Delay::from_numer_denom_ms(ms, 1);
                encoder
                    .encode_frame(ImageFrame::from_parts(image, 0, 0, delay))
                    .unwrap();
            }
        }
        bytes
    }

    #[test]
    fn decodes_a_gif_with_its_delays() {
        let animation = Animation::decode(&gif(&[100, 200, 300])).unwrap().unwrap();
        assert_eq!(animation.len(), 3);
        assert_eq!(animation.duration(), Duration::from_millis(600));
        assert_eq!(animation.frame(1).width(), 4);
    }

    #[test]
    fn decodes_apng_and_animated_webp() {
        for name in ["anim.png", "anim.webp"] {
            let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
            let animation = Animation::decode(&std::fs::read(path).unwrap()).unwrap();
            let animation = animation.unwrap_or_else(|| panic!("{name} is not seen as animated"));
            assert_eq!(animation.len(), 3, "{name}");
            assert_eq!(animation.duration(), Duration::from_millis(600), "{name}");
        }
    }

    #[test]
    fn still_images_are_not_animations() {
        assert!(Animation::decode(&gif(&[100])).unwrap().is_none());
        let mut png = Vec::new();
        RgbaImage::new(2, 2)
            .write_to(&mut Cursor::new(&mut png), FileFormat::Png)
            .unwrap();
        assert!(Animation::decode(&png).unwrap().is_none());
        assert!(Animation::decode(b"not an image").unwrap().is_none());
    }

    #[test]
    fn unspecified_delays_play_at_the_default_speed() {
        let animation = Animation::decode(&gif(&[0, 10, 50])).unwrap().unwrap();
        assert_eq!(animation.duration(), Duration::from_millis(250));
    }

    #[test]
    fn picks_the_frame_for_the_elapsed_time() {
        let animation = Animation::decode(&gif(&[100, 200, 300])).unwrap().unwrap();
        let ms = Duration::from_millis;
        assert_eq!(animation.at(ms(0)), (0, ms(100)));
        assert_eq!(animation.at(ms(150)), (1, ms(150)));
        assert_eq!(animation.at(ms(599)), (2, ms(1)));
        // It loops.
        assert_eq!(animation.at(ms(650)), (0, ms(50)));
    }

    #[test]
    fn refuses_too_many_frames() {
        let err = Animation::decode(&gif(&[20; MAX_FRAMES + 1])).unwrap_err();
        assert!(err.to_string().contains("too long"), "{err}");
    }
}
