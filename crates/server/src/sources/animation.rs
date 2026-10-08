use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use image::DynamicImage;
use ssp_core::{Animation, Fit, Frame};

use super::Source;

/// Plays an [`Animation`] in a loop, each frame for its own delay.
///
/// Which frame is shown follows the clock, not a frame counter: if drawing or sending is slow,
/// frames are skipped instead of the animation slowing down. Long animations, which are not kept
/// decoded, are decoded on a separate thread a few frames ahead.
pub struct Player {
    animation: Arc<Animation>,
    fit: Fit,
    started: Option<Instant>,
    until_next: Duration,
    /// The frame on screen, fitted to the panel, with its number (see `sequence_at`).
    fitted: Option<(u64, Frame)>,
    stream: Option<Stream>,
}

impl Player {
    /// Starts at the first frame on the first [`Source::render`].
    pub fn new(animation: Arc<Animation>, fit: Fit) -> Self {
        Self {
            animation,
            fit,
            started: None,
            until_next: Duration::ZERO,
            fitted: None,
            stream: None,
        }
    }
}

impl Source for Player {
    fn render(&mut self, frame: &mut Frame) {
        let started = *self.started.get_or_insert_with(Instant::now);
        let (sequence, until_next) = self.animation.sequence_at(started.elapsed());
        self.until_next = until_next;
        let (width, height) = (frame.width(), frame.height());
        if let Some((shown, fitted)) = &self.fitted
            && *shown == sequence
            && fitted.width() == width
        {
            frame.clone_from(fitted);
            return;
        }
        let index = (sequence % self.animation.len() as u64) as usize;
        let image = match self.animation.cached(index) {
            Some(image) => Some(image),
            None => self
                .stream
                .get_or_insert_with(|| Stream::new(self.animation.stream()))
                .advance(sequence),
        };
        let Some(image) = image else {
            // The decoder failed: keep showing what is on screen.
            return;
        };
        let fitted = Frame::fit(image, width, height, self.fit);
        frame.clone_from(&fitted);
        self.fitted = Some((sequence, fitted));
    }

    fn next_change(&self) -> Option<Duration> {
        Some(self.until_next)
    }
}

/// Frames of a long animation, decoded on another thread.
struct Stream {
    frames: mpsc::Receiver<ssp_core::Result<(u64, DynamicImage)>>,
    current: Option<(u64, DynamicImage)>,
    /// A frame received that is not due yet.
    ahead: Option<(u64, DynamicImage)>,
}

impl Stream {
    fn new(frames: mpsc::Receiver<ssp_core::Result<(u64, DynamicImage)>>) -> Self {
        Self {
            frames,
            current: None,
            ahead: None,
        }
    }

    /// Takes frames in order up to number `target`, skipping the ones that are late. Waits
    /// while the decoder is behind.
    fn advance(&mut self, target: u64) -> Option<&DynamicImage> {
        loop {
            if self.current.as_ref().is_some_and(|(n, _)| *n >= target) {
                break;
            }
            let next = match self.ahead.take() {
                Some(next) => next,
                None => match self.frames.recv() {
                    Ok(Ok(next)) => next,
                    Ok(Err(err)) => {
                        tracing::warn!("cannot decode the animation: {err}");
                        break;
                    }
                    Err(_) => break,
                },
            };
            if next.0 > target && self.current.is_some() {
                self.ahead = Some(next);
                break;
            }
            self.current = Some(next);
        }
        self.current.as_ref().map(|(_, image)| image)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame as ImageFrame, Rgba, RgbaImage};

    use super::*;

    fn red_then_blue() -> Arc<Animation> {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(Cursor::new(&mut bytes));
            encoder.set_repeat(Repeat::Infinite).unwrap();
            for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let image = RgbaImage::from_pixel(4, 2, Rgba(color));
                let delay = Delay::from_numer_denom_ms(50, 1);
                encoder
                    .encode_frame(ImageFrame::from_parts(image, 0, 0, delay))
                    .unwrap();
            }
        }
        Arc::new(Animation::decode(&bytes).unwrap().unwrap())
    }

    #[test]
    fn shows_the_frames_in_turn() {
        for animation in [
            red_then_blue(),
            Arc::new(Arc::unwrap_or_clone(red_then_blue()).into_streamed()),
        ] {
            plays_red_then_blue(Player::new(animation, Fit::Stretch));
        }
    }

    fn plays_red_then_blue(mut player: Player) {
        let mut frame = Frame::blank(8, 4);
        player.render(&mut frame);
        let first = frame.image().get_pixel(4, 2).0;
        assert!(first[0] > 200 && first[2] < 50, "{first:?}");
        assert!(player.next_change().unwrap() <= Duration::from_millis(50));

        std::thread::sleep(player.next_change().unwrap() + Duration::from_millis(5));
        player.render(&mut frame);
        let second = frame.image().get_pixel(4, 2).0;
        assert!(second[2] > 200 && second[0] < 50, "{second:?}");
    }
}
