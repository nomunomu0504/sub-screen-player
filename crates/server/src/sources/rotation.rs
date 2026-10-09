//! Several contents in turn (`[rotation]`).
//!
//! Only the current turn's source runs: a web page's browser or a video's ffmpeg starts when its
//! turn comes and stops after it. Until a new turn draws its first frame (a web page needs about
//! a second), the last picture of the previous turn stays.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ssp_core::Frame;

use super::{Content, Source};

/// One content and how long it is shown.
#[derive(Clone)]
pub struct Turn {
    /// What is shown.
    pub content: Content,
    /// For how long.
    pub duration: Duration,
}

/// Shows the turns one after another, from the first, in a loop.
pub struct Rotation {
    turns: Arc<Vec<Turn>>,
    index: usize,
    current: Option<Box<dyn Source>>,
    /// When the current turn ends; set when it starts.
    ends: Option<Instant>,
    last: Option<Frame>,
}

impl Rotation {
    /// Starts with the first turn when it first draws.
    pub fn new(turns: Arc<Vec<Turn>>) -> Self {
        Self {
            turns,
            index: 0,
            current: None,
            ends: None,
            last: None,
        }
    }

    /// The index of the turn being shown.
    #[cfg(test)]
    fn turn(&self) -> usize {
        self.index
    }
}

impl Source for Rotation {
    fn render(&mut self, frame: &mut Frame) {
        if self.turns.is_empty() {
            return;
        }
        let now = Instant::now();
        match self.ends {
            None => {
                self.current = self.turns[self.index].content.source();
                self.ends = Some(now + self.turns[self.index].duration);
            }
            Some(ends) if now >= ends => {
                // Stop the old source before starting the next (a browser, ffmpeg).
                self.current = None;
                self.index = (self.index + 1) % self.turns.len();
                self.current = self.turns[self.index].content.source();
                self.ends = Some(now + self.turns[self.index].duration);
            }
            Some(_) => {}
        }
        if let Some(last) = &self.last {
            frame.clone_from(last);
        }
        if let Some(current) = &mut self.current {
            current.render(frame);
        }
        self.last = Some(frame.clone());
    }

    fn next_change(&self) -> Option<Duration> {
        let Some(ends) = self.ends else {
            return (!self.turns.is_empty()).then_some(Duration::ZERO);
        };
        let left = ends.saturating_duration_since(Instant::now());
        Some(match self.current.as_ref().and_then(|c| c.next_change()) {
            Some(next) => next.min(left),
            None => left,
        })
    }
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, Rgb, RgbImage};
    use ssp_core::Fit;

    use super::*;

    fn color(rgb: [u8; 3], millis: u64) -> Turn {
        Turn {
            content: Content::Image {
                image: Arc::new(DynamicImage::ImageRgb8(RgbImage::from_pixel(
                    4,
                    2,
                    Rgb(rgb),
                ))),
                fit: Fit::Stretch,
            },
            duration: Duration::from_millis(millis),
        }
    }

    #[test]
    fn shows_each_turn_for_its_time_in_a_loop() {
        let turns = Arc::new(vec![color([255, 0, 0], 60), color([0, 0, 255], 60)]);
        let mut rotation = Rotation::new(turns);
        let mut frame = Frame::blank(8, 4);
        let mut seen = Vec::new();
        // Until the first turn came back; slow machines take longer, so no fixed time.
        let started = Instant::now();
        while seen.len() < 3 {
            assert!(started.elapsed() < Duration::from_secs(10), "{seen:?}");
            rotation.render(&mut frame);
            let pixel = frame.image().get_pixel(1, 1).0;
            if seen.last() != Some(&pixel) {
                seen.push(pixel);
            }
            let wait = rotation.next_change().unwrap();
            assert!(wait <= Duration::from_millis(60), "{wait:?}");
            std::thread::sleep(wait.max(Duration::from_millis(5)));
        }
        assert_eq!(seen[0], [255, 0, 0]);
        assert_eq!(seen[1], [0, 0, 255]);
        assert_eq!(seen[2], [255, 0, 0]);
    }

    #[test]
    fn keeps_the_last_picture_when_a_turn_draws_nothing() {
        let nothing = Turn {
            content: Content::Nothing,
            duration: Duration::from_secs(60),
        };
        let turns = Arc::new(vec![color([0, 255, 0], 1), nothing]);
        let mut rotation = Rotation::new(turns);
        let mut frame = Frame::blank(8, 4);
        rotation.render(&mut frame);
        std::thread::sleep(Duration::from_millis(5));
        let mut next = Frame::blank(8, 4);
        rotation.render(&mut next);
        assert_eq!(rotation.turn(), 1);
        assert_eq!(next.image().get_pixel(1, 1).0, [0, 255, 0]);
    }
}
