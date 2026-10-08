//! Built-in sources: things that draw frames by themselves inside the daemon.
//!
//! A new built-in screen is a new module here implementing [`Source`], plus a [`Content`]
//! variant that creates it. Frames from outside the daemon (HTTP, WebSocket) do not need a
//! source; they are submitted to the display directly.

mod clock;
mod dashboard;
pub mod stats;

use std::sync::Arc;
use std::time::Duration;

use image::DynamicImage;
use ssp_core::{Fit, Frame};

pub use clock::Clock;
pub use dashboard::Dashboard;

use crate::config::{ClockConfig, DashboardConfig};

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
    /// The built-in clock.
    Clock(ClockConfig),
    /// The built-in dashboard; its clock panel uses the clock settings.
    Dashboard(DashboardConfig, ClockConfig),
    /// Frames from a WebSocket client.
    Stream,
}

impl Content {
    /// Short name used by the API.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Image { .. } => "image",
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
            Self::Clock(config) => Some(Box::new(Clock::new(config.clone()))),
            Self::Dashboard(dashboard, clock) => {
                Some(Box::new(Dashboard::new(dashboard.clone(), clock.clone())))
            }
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
