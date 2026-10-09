//! Several contents side by side (`[layout]`).
//!
//! Each zone runs its own source on a thread of its own, at the zone's size, so that a video or
//! a web page waiting for its next frame does not hold the others back. The layout puts the
//! zones' latest pictures together whenever one of them changes. Zone edges are on the 16-pixel
//! grid, so that with partial updates the change of one zone is sent as a part of its own.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use image::{GenericImage, Rgb};
use ssp_core::Frame;

use super::{Content, Source};

/// Zone edges are multiples of this many pixels (see [`ssp_core::regions::TILE`]).
const GRID: u32 = 16;
/// How long [`Layout::render`] waits for a zone to change before drawing again anyway.
const WAIT: Duration = Duration::from_secs(1);

/// One zone: what it shows and how wide it is.
#[derive(Clone)]
pub struct Zone {
    /// What the zone shows.
    pub content: Content,
    /// A share of the panel's width (up to 1) or pixels (more than 1); `None` shares the rest.
    pub width: Option<f64>,
}

/// The zones of a layout and its look.
pub struct LayoutSpec {
    /// From left to right.
    pub zones: Vec<Zone>,
    /// Pixels between zones.
    pub gap: u32,
    /// Color around and between the zones.
    pub background: [u8; 3],
}

/// Where the zones of `widths` go on a panel `width` pixels wide: the left edge and the width of
/// each, on the 16-pixel grid.
///
/// Fixed widths are taken first (scaled down together if they do not fit); zones without one
/// share what is left. What no zone takes stays background, on the right.
pub fn columns(widths: &[Option<f64>], gap: u32, width: u32) -> Vec<(u32, u32)> {
    let n = widths.len();
    if n == 0 {
        return Vec::new();
    }
    let room = f64::from(width.saturating_sub(gap * (n as u32 - 1)));
    let fixed: Vec<Option<f64>> = widths
        .iter()
        .map(|w| w.map(|w| if w <= 1.0 { w * room } else { w }))
        .collect();
    let taken: f64 = fixed.iter().flatten().sum();
    let scale = if taken > room { room / taken } else { 1.0 };
    let free = fixed.iter().filter(|w| w.is_none()).count();
    let share = if free > 0 {
        (room - taken * scale).max(0.0) / free as f64
    } else {
        0.0
    };
    let snap = |x: f64| ((x / f64::from(GRID)).round() as u32 * GRID).min(width);
    let mut x = 0.0;
    let mut placed = Vec::with_capacity(n);
    for (i, w) in fixed.iter().enumerate() {
        let w = w.map_or(share, |w| w * scale);
        let start = snap(x);
        // The last zone reaches the edge if it fills the panel.
        let mut end = if i + 1 == n && (x + w - f64::from(width)).abs() < 1.0 {
            width
        } else {
            snap(x + w)
        };
        if end <= start {
            end = (start + GRID).min(width);
        }
        placed.push((start, end - start));
        x += w + f64::from(gap);
    }
    placed
}

/// Shows a [`LayoutSpec`].
pub struct Layout {
    spec: Arc<LayoutSpec>,
    running: Option<Running>,
    drawn: u64,
}

impl Layout {
    /// Starts the zones when it first draws, at the panel's size.
    pub fn new(spec: Arc<LayoutSpec>) -> Self {
        Self {
            spec,
            running: None,
            drawn: 0,
        }
    }
}

impl Source for Layout {
    fn render(&mut self, frame: &mut Frame) {
        let size = (frame.width(), frame.height());
        if self.running.as_ref().is_none_or(|r| r.size != size) {
            // Stop the old zones (a browser, ffmpeg) before starting new ones.
            self.running = None;
            self.running = Some(Running::start(&self.spec, size));
            self.drawn = 0;
        }
        let running = self.running.as_ref().expect("started above");
        let pictures = running.wait_for(self.drawn);
        self.drawn = pictures.version;
        let image = frame.image_mut();
        image
            .pixels_mut()
            .for_each(|p| *p = Rgb(self.spec.background));
        for (picture, (x, _)) in pictures.frames.iter().zip(&running.columns) {
            if let Some(picture) = picture {
                // Drawn at the zone's size, so it fits.
                let _ = image.copy_from(picture.image(), *x, 0);
            }
        }
    }

    fn next_change(&self) -> Option<Duration> {
        // `render` waits for a zone to change.
        Some(Duration::ZERO)
    }
}

/// The zones' threads for one panel size.
struct Running {
    size: (u32, u32),
    columns: Vec<(u32, u32)>,
    shared: Arc<Shared>,
    zones: Vec<ZoneThread>,
}

#[derive(Default)]
struct Shared {
    pictures: Mutex<Pictures>,
    changed: Condvar,
}

#[derive(Default)]
struct Pictures {
    /// The latest picture of each zone.
    frames: Vec<Option<Frame>>,
    /// Bumped by every new picture.
    version: u64,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Pictures> {
        self.pictures.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl Running {
    fn start(spec: &LayoutSpec, (width, height): (u32, u32)) -> Self {
        let widths: Vec<Option<f64>> = spec.zones.iter().map(|z| z.width).collect();
        let columns = columns(&widths, spec.gap, width);
        let shared = Arc::new(Shared::default());
        shared.lock().frames = vec![None; spec.zones.len()];
        let zones = spec
            .zones
            .iter()
            .zip(&columns)
            .enumerate()
            .filter_map(|(index, (zone, (_, w)))| {
                let source = zone.content.source()?;
                Some(ZoneThread::start(
                    index,
                    source,
                    (*w, height),
                    shared.clone(),
                ))
            })
            .collect();
        Self {
            size: (width, height),
            columns,
            shared,
            zones,
        }
    }

    /// The pictures once one is newer than `version`, or after [`WAIT`].
    fn wait_for(&self, version: u64) -> MutexGuard<'_, Pictures> {
        let pictures = self.shared.lock();
        self.shared
            .changed
            .wait_timeout_while(pictures, WAIT, |p| p.version == version)
            .map_or_else(|p| p.into_inner().0, |(pictures, _)| pictures)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Ask every zone to stop first, then wait, so slow ones stop together.
        let threads: Vec<_> = self.zones.drain(..).map(ZoneThread::ask_to_stop).collect();
        for thread in threads.into_iter().flatten() {
            let _ = thread.join();
        }
    }
}

/// Runs one zone's source, like the display's player thread does for a whole panel.
struct ZoneThread {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl ZoneThread {
    fn start(
        index: usize,
        mut source: Box<dyn Source>,
        (width, height): (u32, u32),
        shared: Arc<Shared>,
    ) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name(format!("ssp-zone-{index}"))
            .spawn(move || {
                let mut frame = Frame::blank(width, height);
                loop {
                    source.render(&mut frame);
                    {
                        let mut pictures = shared.lock();
                        pictures.frames[index] = Some(frame.clone());
                        pictures.version += 1;
                    }
                    shared.changed.notify_all();
                    let Some(wait) = source.next_change() else {
                        break;
                    };
                    if stopped.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            });
        if let Err(err) = &thread {
            tracing::warn!("cannot start a layout zone: {err}");
        }
        Self {
            stop,
            thread: thread.ok(),
        }
    }

    fn ask_to_stop(self) -> Option<JoinHandle<()>> {
        drop(self.stop);
        self.thread
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use image::{DynamicImage, RgbImage};
    use ssp_core::Fit;

    use super::*;
    use crate::config::ClockConfig;

    #[test]
    fn places_zones_on_the_grid() {
        // Shares, pixels and the rest.
        assert_eq!(
            columns(&[Some(0.3), None, Some(480.0)], 0, 1920),
            [(0, 576), (576, 864), (1440, 480)]
        );
        // Zones without a width share the panel; the last one reaches the edge.
        assert_eq!(
            columns(&[None, None, None], 0, 1920),
            [(0, 640), (640, 640), (1280, 640)]
        );
        // A gap between zones.
        assert_eq!(columns(&[None, None], 32, 1920), [(0, 944), (976, 944)]);
        // Too wide: scaled down together.
        assert_eq!(
            columns(&[Some(1200.0), Some(1200.0)], 0, 1920),
            [(0, 960), (960, 960)]
        );
        // Less than the panel: background on the right.
        assert_eq!(columns(&[Some(0.5)], 0, 1920), [(0, 960)]);
        // Every edge is on the grid.
        for (x, w) in columns(&[Some(0.33), Some(0.33), None], 8, 1920) {
            assert_eq!((x % GRID, (x + w) % GRID), (0, 0), "{x} {w}");
        }
    }

    fn color(rgb: [u8; 3]) -> Content {
        Content::Image {
            image: Arc::new(DynamicImage::ImageRgb8(RgbImage::from_pixel(
                4,
                4,
                Rgb(rgb),
            ))),
            fit: Fit::Stretch,
        }
    }

    #[test]
    fn puts_the_zones_side_by_side() {
        let spec = Arc::new(LayoutSpec {
            zones: vec![
                Zone {
                    content: color([255, 0, 0]),
                    width: Some(0.25),
                },
                Zone {
                    content: Content::Nothing,
                    width: Some(0.25),
                },
                Zone {
                    content: color([0, 0, 255]),
                    width: None,
                },
            ],
            gap: 0,
            background: [9, 9, 9],
        });
        let mut layout = Layout::new(spec);
        let mut frame = Frame::blank(1920, 462);
        let started = Instant::now();
        loop {
            layout.render(&mut frame);
            let image = frame.image();
            if image.get_pixel(10, 10).0 == [255, 0, 0]
                && image.get_pixel(1900, 400).0 == [0, 0, 255]
            {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "zones not drawn"
            );
        }
        assert_eq!(
            frame.image().get_pixel(700, 200).0,
            [9, 9, 9],
            "an empty zone"
        );
        assert_eq!(layout.next_change(), Some(Duration::ZERO));
    }

    #[test]
    fn a_clock_zone_keeps_ticking() {
        let spec = Arc::new(LayoutSpec {
            zones: vec![Zone {
                content: Content::Clock(ClockConfig::default()),
                width: Some(0.5),
            }],
            gap: 0,
            background: [0, 0, 0],
        });
        let mut layout = Layout::new(spec);
        let mut frame = Frame::blank(1920, 462);
        layout.render(&mut frame);
        let first = layout.drawn;
        // The clock draws again within a second or so.
        let started = Instant::now();
        while layout.drawn == first {
            layout.render(&mut frame);
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the clock stopped"
            );
        }
        // Nothing is drawn right of the zone.
        assert_eq!(frame.image().get_pixel(1500, 230).0, [0, 0, 0]);
    }
}
