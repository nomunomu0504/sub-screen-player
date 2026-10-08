use std::sync::Arc;
use std::time::{Duration, Instant};

use ssp_core::{Animation, Fit, Frame};

use super::Source;

/// Plays an [`Animation`] in a loop, each frame for its own delay.
///
/// Which frame is shown follows the clock, not a frame counter: if drawing or sending is slow,
/// frames are skipped instead of the animation slowing down.
pub struct Player {
    animation: Arc<Animation>,
    fit: Fit,
    started: Option<Instant>,
    /// The last frame fitted to the panel, so a frame shown twice is not scaled twice.
    fitted: Option<(usize, Frame)>,
    until_next: Duration,
}

impl Player {
    /// Starts at the first frame on the first [`Source::render`].
    pub fn new(animation: Arc<Animation>, fit: Fit) -> Self {
        Self {
            animation,
            fit,
            started: None,
            fitted: None,
            until_next: Duration::ZERO,
        }
    }
}

impl Source for Player {
    fn render(&mut self, frame: &mut Frame) {
        let started = *self.started.get_or_insert_with(Instant::now);
        let (index, until_next) = self.animation.at(started.elapsed());
        self.until_next = until_next;
        let (width, height) = (frame.width(), frame.height());
        let fitted = match self.fitted.take() {
            Some((shown, fitted)) if shown == index && fitted.width() == width => fitted,
            _ => Frame::fit(self.animation.frame(index), width, height, self.fit),
        };
        frame.clone_from(&fitted);
        self.fitted = Some((index, fitted));
    }

    fn next_change(&self) -> Option<Duration> {
        Some(self.until_next)
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
        let mut player = Player::new(red_then_blue(), Fit::Stretch);
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
