//! Built-in sources: things that draw frames by themselves inside the daemon.
//!
//! A new built-in screen is a new module here implementing [`Source`], plus a [`Content`]
//! variant that creates it. Frames from outside the daemon (HTTP, WebSocket) do not need a
//! source; they are submitted to the display directly.

mod animation;
mod clock;
mod dashboard;
pub(crate) use dashboard::number;
pub mod stats;
pub mod video;
pub mod web;

use std::sync::Arc;
use std::time::Duration;

use image::DynamicImage;
use ssp_core::{Animation, Fit, Frame};

pub use clock::Clock;
pub use dashboard::Dashboard;

use crate::config::{ClockConfig, DashboardConfig};
use crate::metrics::Metrics;

/// Something that draws a picture and knows when it changes.
pub trait Source: Send {
    /// Draws the current picture into `frame` (landscape, panel-sized).
    fn render(&mut self, frame: &mut Frame);

    /// How long until the picture changes. `None` if it never does.
    fn next_change(&self) -> Option<Duration>;
}

/// What a display is told to show.
#[derive(Clone)]
pub enum Content {
    /// Nothing is sent; the screen keeps what it has.
    Nothing,
    /// A still image.
    Image {
        /// The image at its original size.
        image: Arc<DynamicImage>,
        /// How it is fitted to the panel.
        fit: Fit,
    },
    /// An animated GIF, APNG or WebP, played in a loop.
    Animation {
        /// The frames at their original size.
        animation: Arc<Animation>,
        /// How each frame is fitted to the panel.
        fit: Fit,
    },
    /// A video file, played in a loop by ffmpeg.
    Video {
        /// The file.
        video: Arc<video::VideoFile>,
        /// How each frame is fitted to the panel.
        fit: Fit,
    },
    /// A web page drawn by headless Chrome.
    Web {
        /// The page.
        page: web::WebPage,
        /// The browser settings.
        web: crate::web::Web,
    },
    /// The built-in clock.
    Clock(ClockConfig),
    /// The built-in dashboard; its clock panel uses the clock settings and its `metric:<id>`
    /// panels the metrics sent to the daemon.
    Dashboard(DashboardConfig, ClockConfig, Metrics),
    /// Frames from a WebSocket client.
    Stream,
}

impl Content {
    /// Short name used by the API.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Image { .. } => "image",
            Self::Animation { .. } => "animation",
            Self::Video { .. } => "video",
            Self::Web { .. } => "web",
            Self::Clock(_) => "clock",
            Self::Dashboard(..) => "dashboard",
            Self::Stream => "stream",
        }
    }

    /// The source that produces this content, if it is drawn inside the daemon.
    pub fn source(&self) -> Option<Box<dyn Source>> {
        match self {
            Self::Nothing | Self::Stream => None,
            Self::Image { image, fit } => Some(Box::new(Still {
                image: image.clone(),
                fit: *fit,
            })),
            Self::Animation { animation, fit } => {
                Some(Box::new(animation::Player::new(animation.clone(), *fit)))
            }
            Self::Video { video, fit } => Some(Box::new(video::Player::new(video.clone(), *fit))),
            Self::Web { page, web } => Some(Box::new(web::Page::new(page.clone(), web.clone()))),
            Self::Clock(config) => Some(Box::new(Clock::new(config.clone()))),
            Self::Dashboard(dashboard, clock, metrics) => Some(Box::new(Dashboard::new(
                dashboard.clone(),
                clock.clone(),
                metrics.clone(),
            ))),
        }
    }
}

/// Shows one image.
struct Still {
    image: Arc<DynamicImage>,
    fit: Fit,
}

impl Source for Still {
    fn render(&mut self, frame: &mut Frame) {
        *frame = Frame::fit(&self.image, frame.width(), frame.height(), self.fit);
    }

    fn next_change(&self) -> Option<Duration> {
        None
    }
}

/// A decoded image file: a still picture or an animation.
pub enum Picture {
    /// A still image (or an animated file with a single frame).
    Still(DynamicImage),
    /// An animated GIF, APNG or WebP with at least two frames.
    Animated(Animation),
}

impl Picture {
    /// Decodes a PNG, JPEG, GIF or WebP file; animated GIF, APNG and WebP become animations.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if let Some(animation) =
            Animation::decode(bytes).map_err(|e| format!("cannot decode image: {e}"))?
        {
            return Ok(Self::Animated(animation));
        }
        image::load_from_memory(bytes)
            .map(Self::Still)
            .map_err(|e| format!("cannot decode image: {e}"))
    }

    /// The still image, or the first frame of the animation.
    pub fn first(&self) -> &DynamicImage {
        match self {
            Self::Still(image) => image,
            Self::Animated(animation) => animation.first(),
        }
    }

    /// The content that shows this picture.
    pub fn into_content(self, fit: Fit) -> Content {
        match self {
            Self::Still(image) => Content::Image {
                image: Arc::new(image),
                fit,
            },
            Self::Animated(animation) => Content::Animation {
                animation: Arc::new(animation),
                fit,
            },
        }
    }
}
