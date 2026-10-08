use std::io::Cursor;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, DynamicImage, Frames, ImageFormat as FileFormat};

use crate::{Error, Result};

/// Most frames an animation may have (about 2.5 minutes at 60 fps).
pub const MAX_FRAMES: usize = 10_000;
/// Animations whose decoded frames (8-bit RGBA) fit in this much memory are kept decoded; longer
/// ones are decoded again frame by frame while they play (see [`Animation::stream`]).
pub const MAX_CACHED_BYTES: usize = 256 << 20;
/// Delays up to this are played as [`DEFAULT_DELAY`], as browsers do: many GIFs say 0 or 10 ms
/// and mean "the default speed". 60 fps animations (about 16.7 ms) play at their own speed.
const MAX_UNSET_DELAY: Duration = Duration::from_millis(10);
/// See [`MAX_UNSET_DELAY`].
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// An animated image (GIF, APNG or animated WebP): its frames at their original size, each with
/// the time it stays on screen. Playback loops forever.
///
/// Short animations keep all frames decoded. Long ones (a full-screen 60 fps animation passes
/// [`MAX_CACHED_BYTES`] after about a second) keep only the file and are decoded again while
/// playing, a few frames ahead.
#[derive(Debug, Clone)]
pub struct Animation {
    file: Arc<[u8]>,
    format: FileFormat,
    delays: Vec<Duration>,
    total: Duration,
    first: DynamicImage,
    /// All frames, when they fit in [`MAX_CACHED_BYTES`].
    frames: Option<Arc<Vec<DynamicImage>>>,
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
        let file: Arc<[u8]> = bytes.into();
        let Some(frames) = frames_of(&file, format)? else {
            return Ok(None);
        };
        let mut delays = Vec::new();
        let mut cached = Some(Vec::new());
        let mut cached_bytes = 0;
        let mut first = None;
        for frame in frames {
            let frame = frame.map_err(image)?;
            if delays.len() == MAX_FRAMES {
                return Err(Error::InvalidArgument(format!(
                    "the animation is too long: at most {MAX_FRAMES} frames"
                )));
            }
            delays.push(delay_of(&frame));
            let image = DynamicImage::ImageRgba8(frame.into_buffer());
            if first.is_none() {
                first = Some(image.clone());
            }
            cached_bytes += image.as_bytes().len();
            if cached_bytes > MAX_CACHED_BYTES {
                cached = None;
            }
            if let Some(cached) = &mut cached {
                cached.push(image);
            }
        }
        let Some(first) = first.filter(|_| delays.len() >= 2) else {
            return Ok(None);
        };
        Ok(Some(Self {
            file,
            format,
            total: delays.iter().sum(),
            delays,
            first,
            frames: cached.map(Arc::new),
        }))
    }

    /// Number of frames (at least 2).
    pub fn len(&self) -> usize {
        self.delays.len()
    }

    /// Always `false`: an animation has at least two frames.
    pub fn is_empty(&self) -> bool {
        self.delays.is_empty()
    }

    /// The first frame at its original size.
    pub fn first(&self) -> &DynamicImage {
        &self.first
    }

    /// Frame `index` at its original size, if the frames are kept decoded (see
    /// [`MAX_CACHED_BYTES`]); otherwise play it with [`Animation::stream`].
    pub fn cached(&self, index: usize) -> Option<&DynamicImage> {
        self.frames.as_ref().and_then(|frames| frames.get(index))
    }

    /// Drops the decoded frames, so playing decodes them again as it goes, like a long
    /// animation. Saves memory; mostly useful for tests.
    pub fn into_streamed(mut self) -> Self {
        self.frames = None;
        self
    }

    /// How long one pass through all frames takes.
    pub fn duration(&self) -> Duration {
        self.total
    }

    /// The frame to show `elapsed` after playback started, and how long until the next one.
    pub fn at(&self, elapsed: Duration) -> (usize, Duration) {
        let total = self.total.as_nanos().max(1);
        let mut position = Duration::from_nanos((elapsed.as_nanos() % total) as u64);
        for (index, delay) in self.delays.iter().enumerate() {
            if position < *delay {
                return (index, *delay - position);
            }
            position -= *delay;
        }
        // Only reachable through rounding: start over.
        (0, self.delays[0])
    }

    /// Like [`Animation::at`], but counting frames across loops: the second pass through a
    /// 10-frame animation starts at 10. Matches the numbers from [`Animation::stream`].
    pub fn sequence_at(&self, elapsed: Duration) -> (u64, Duration) {
        let loops = (elapsed.as_nanos() / self.total.as_nanos().max(1)) as u64;
        let (index, until_next) = self.at(elapsed);
        (loops * self.len() as u64 + index as u64, until_next)
    }

    /// Decodes the frames on a new thread, in order and looping forever, a few ahead of the
    /// consumer. Each frame comes with its number counted across loops (see
    /// [`Animation::sequence_at`]). The thread ends when the receiver is dropped.
    pub fn stream(&self) -> mpsc::Receiver<Result<(u64, DynamicImage)>> {
        let (send, receive) = mpsc::sync_channel(2);
        let file = self.file.clone();
        let format = self.format;
        std::thread::Builder::new()
            .name("ssp-animation".into())
            .spawn(move || {
                let mut sequence = 0u64;
                loop {
                    let frames = match frames_of(&file, format) {
                        Ok(Some(frames)) => frames,
                        Ok(None) => return,
                        Err(err) => {
                            let _ = send.send(Err(err));
                            return;
                        }
                    };
                    for frame in frames {
                        let item = frame
                            .map(|f| (sequence, DynamicImage::ImageRgba8(f.into_buffer())))
                            .map_err(image);
                        let failed = item.is_err();
                        if send.send(item).is_err() || failed {
                            return;
                        }
                        sequence += 1;
                    }
                }
            })
            .expect("failed to spawn a thread");
        receive
    }
}

/// The frame decoder for an animated file, or `None` if it is not animated.
fn frames_of(file: &Arc<[u8]>, format: FileFormat) -> Result<Option<Frames<'static>>> {
    let reader = Cursor::new(file.clone());
    Ok(Some(match format {
        FileFormat::Gif => GifDecoder::new(reader).map_err(image)?.into_frames(),
        FileFormat::Png => {
            let decoder = PngDecoder::new(reader).map_err(image)?;
            if !decoder.is_apng().map_err(image)? {
                return Ok(None);
            }
            decoder.apng().map_err(image)?.into_frames()
        }
        FileFormat::WebP => {
            let decoder = WebPDecoder::new(reader).map_err(image)?;
            if !decoder.has_animation() {
                return Ok(None);
            }
            decoder.into_frames()
        }
        _ => return Ok(None),
    }))
}

fn delay_of(frame: &image::Frame) -> Duration {
    match Duration::from(frame.delay()) {
        unset if unset <= MAX_UNSET_DELAY => DEFAULT_DELAY,
        delay => delay,
    }
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
        assert_eq!(animation.cached(1).unwrap().width(), 4);
        assert_eq!(animation.first().width(), 4);
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
    fn fast_animations_keep_their_speed() {
        // GIF delays are in 10 ms steps: 20 ms is the fastest real speed (50 fps).
        let animation = Animation::decode(&gif(&[20, 20, 20])).unwrap().unwrap();
        assert_eq!(animation.duration(), Duration::from_millis(60));
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

    #[test]
    fn streams_frames_in_order_across_loops() {
        let animation = Animation::decode(&gif(&[100, 200, 300])).unwrap().unwrap();
        let frames = animation.stream();
        let numbers: Vec<u64> = (0..7).map(|_| frames.recv().unwrap().unwrap().0).collect();
        assert_eq!(numbers, [0, 1, 2, 3, 4, 5, 6]);
        let ms = Duration::from_millis;
        assert_eq!(animation.sequence_at(ms(650)), (3, ms(50)));
        assert_eq!(animation.sequence_at(ms(1250)), (6, ms(50)));
    }
}
