//! Notifications: a message drawn over whatever a display shows, for a while (`ssp notify`).
//!
//! A notification belongs to a display, not to its content: it stays when the content changes
//! or the display is unplugged and plugged in again, and it ends on its own timer. Every frame a
//! display gets passes through the display's [`Overlay`] (see
//! [`crate::manager::Device::submit`]), which keeps the frame and draws the notification on top.
//! When a notification starts or ends, the overlay shows the last frame again with or without
//! it, so it also works over a still image, a stopped content or nothing at all (black).

use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use image::{GenericImage, RgbImage};
use serde::{Deserialize, Serialize};
use ssp_core::Frame;

use crate::manager::Device;
use crate::text::{TextStyle, builtin_font};

/// Longest title, in characters.
pub const MAX_TEXT: usize = 80;
/// Longest detail line, in characters.
pub const MAX_DETAIL: usize = 120;
/// How long a notification stays unless told otherwise.
pub const DEFAULT_SECONDS: u64 = 10;
/// Longest time a notification may stay (a day); longer ones are sticky.
pub const MAX_SECONDS: u64 = 24 * 3600;
/// The color unless told otherwise.
pub const DEFAULT_COLOR: &str = "blue";

/// Named colors for `--color`, besides `#rrggbb`.
const COLORS: [(&str, [u8; 3]); 6] = [
    ("red", [0xdc, 0x26, 0x26]),
    ("orange", [0xea, 0x58, 0x0c]),
    ("yellow", [0xea, 0xb3, 0x08]),
    ("green", [0x16, 0xa3, 0x4a]),
    ("blue", [0x25, 0x63, 0xeb]),
    ("gray", [0x4b, 0x55, 0x63]),
];

/// Where a notification is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// A band across the bottom third; the content shows above it.
    #[default]
    Banner,
    /// The whole panel, for things that must not be missed.
    Full,
}

/// A message to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// The title.
    pub text: String,
    /// A smaller line under the title.
    pub detail: Option<String>,
    /// Banner or whole panel.
    pub style: Style,
    /// Background color.
    pub color: [u8; 3],
    /// How long it stays; `None` until it is dismissed or replaced.
    pub duration: Option<Duration>,
    /// Switch a screen that is off on while it is shown (and off again after).
    pub wake: bool,
}

impl Notification {
    /// Checks the lengths of the texts and the duration.
    pub fn validate(&self) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("the text of a notification is empty".into());
        }
        if self.text.chars().count() > MAX_TEXT {
            return Err(format!("the text is longer than {MAX_TEXT} characters"));
        }
        if self
            .detail
            .as_ref()
            .is_some_and(|d| d.chars().count() > MAX_DETAIL)
        {
            return Err(format!("the detail is longer than {MAX_DETAIL} characters"));
        }
        match self.duration {
            Some(d) if d.is_zero() => Err("seconds must be at least 1".into()),
            Some(d) if d > Duration::from_secs(MAX_SECONDS) => Err(format!(
                "seconds must be at most {MAX_SECONDS} (use sticky to keep it until dismissed)"
            )),
            _ => Ok(()),
        }
    }
}

/// `red`, `orange`, `yellow`, `green`, `blue`, `gray` or `#rrggbb`.
pub fn parse_color(name: &str) -> Option<[u8; 3]> {
    COLORS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, rgb)| *rgb)
        .or_else(|| crate::config::parse_color(name))
}

/// A notification being shown, as the API reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// What is shown.
    pub notification: Notification,
    /// Time left; `None` for a sticky one.
    pub left: Option<Duration>,
}

/// The notification of one display and the last frame its content sent. Kept across
/// reconnects; the display's current [`Device`] is attached when it connects.
#[derive(Default)]
pub struct Overlay {
    state: Mutex<State>,
    /// Signalled when the notification changes, so that old timers end.
    changed: Condvar,
}

#[derive(Default)]
struct State {
    current: Option<Current>,
    /// Bumped by every change of `current`, so that an old timer does not end a newer
    /// notification.
    generation: u64,
    /// The last frame of the content, without the notification.
    last: Option<Frame>,
    device: Weak<Device>,
}

struct Current {
    notification: Notification,
    until: Option<Instant>,
    /// Whether showing it switched the screen on (it goes off again at the end).
    woke: bool,
    /// The notification drawn for a panel size: the top row it starts at and its pixels.
    drawn: Option<(u32, RgbImage)>,
}

impl Current {
    /// Draws the notification over `frame`.
    fn draw_over(&mut self, frame: &mut Frame) {
        let (width, height) = (frame.width(), frame.height());
        let stale = self
            .drawn
            .as_ref()
            .is_none_or(|(_, image)| image.width() != width);
        if stale {
            self.drawn = Some(render(&self.notification, width, height));
        }
        let (top, image) = self.drawn.as_ref().expect("drawn above");
        // Fits: rendered for this width and at most this height.
        let _ = frame.image_mut().copy_from(image, 0, *top);
    }
}

impl Overlay {
    /// An overlay without a notification.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Called with every frame on its way to the display: keeps it, and returns it with the
    /// notification drawn on top.
    pub fn pass(&self, frame: Frame) -> Frame {
        let mut state = self.lock();
        let Some(current) = state.current.as_mut() else {
            state.last = Some(frame.clone());
            return frame;
        };
        let mut shown = frame.clone();
        current.draw_over(&mut shown);
        state.last = Some(frame);
        shown
    }

    /// Frames go to `device` from now on (it just connected). A notification being shown is
    /// drawn again over the last frame.
    pub fn attach(&self, device: &Arc<Device>) {
        let frame = {
            let mut state = self.lock();
            state.device = Arc::downgrade(device);
            match state.current.is_some() {
                true => Some(Self::screen(&mut state, device)),
                false => None,
            }
        };
        if let Some(frame) = frame {
            send(device, frame);
        }
    }

    /// Shows `notification` in place of the current one, if any. It ends after its duration.
    pub fn show(self: &Arc<Self>, notification: Notification) {
        let device = self.lock().device.upgrade();
        let woke_now = notification.wake
            && device.as_ref().is_some_and(|d| {
                d.presenter.is_asleep()
                    && d.presenter
                        .wake()
                        .inspect_err(|err| tracing::warn!("cannot switch the screen on: {err}"))
                        .is_ok()
            });
        let until = notification.duration.map(|d| Instant::now() + d);
        let (generation, frame) = {
            let mut state = self.lock();
            let woke = woke_now || state.current.as_ref().is_some_and(|c| c.woke);
            state.current = Some(Current {
                notification,
                until,
                woke,
                drawn: None,
            });
            state.generation += 1;
            let frame = device.as_ref().map(|d| Self::screen(&mut state, d));
            (state.generation, frame)
        };
        self.changed.notify_all();
        if let (Some(device), Some(frame)) = (&device, frame) {
            send(device, frame);
        }
        if let Some(until) = until {
            let overlay = self.clone();
            let spawned = std::thread::Builder::new()
                .name("ssp-notify".into())
                .spawn(move || overlay.end_at(generation, until));
            if let Err(err) = spawned {
                tracing::warn!("cannot time the notification: {err}");
            }
        }
    }

    /// Ends the notification being shown. `false` if there was none.
    pub fn dismiss(&self) -> bool {
        self.end(None)
    }

    /// Drops the notification and the last frame without showing anything (the screen is
    /// being blanked).
    pub fn forget(&self) {
        let mut state = self.lock();
        state.current = None;
        state.last = None;
        state.generation += 1;
        drop(state);
        self.changed.notify_all();
    }

    /// The notification being shown, if any.
    pub fn current(&self) -> Option<Shown> {
        let state = self.lock();
        let current = state.current.as_ref()?;
        Some(Shown {
            notification: current.notification.clone(),
            left: current
                .until
                .map(|until| until.saturating_duration_since(Instant::now())),
        })
    }

    /// Waits until `until`, then ends notification `generation` unless another one came.
    fn end_at(&self, generation: u64, until: Instant) {
        let mut state = self.lock();
        loop {
            if state.generation != generation {
                return;
            }
            let now = Instant::now();
            if now >= until {
                break;
            }
            state = self
                .changed
                .wait_timeout(state, until - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        drop(state);
        self.end(Some(generation));
    }

    /// Ends the notification (only `generation`, if given) and shows the last frame without it.
    fn end(&self, generation: Option<u64>) -> bool {
        let (ended, device, frame) = {
            let mut state = self.lock();
            if generation.is_some_and(|g| g != state.generation) {
                return false;
            }
            let Some(ended) = state.current.take() else {
                return false;
            };
            state.generation += 1;
            let device = state.device.upgrade();
            let frame = device.as_ref().map(|d| Self::screen(&mut state, d));
            (ended, device, frame)
        };
        self.changed.notify_all();
        if let (Some(device), Some(frame)) = (device, frame) {
            send(&device, frame);
            if ended.woke
                && let Err(err) = device.presenter.sleep()
            {
                tracing::warn!("cannot switch the screen off again: {err}");
            }
        }
        true
    }

    /// What the screen should show now: the last frame (black if there is none) with the
    /// notification, if any.
    fn screen(state: &mut State, device: &Device) -> Frame {
        let panel = device.presenter.info().panel;
        let mut frame = state
            .last
            .clone()
            .filter(|f| (f.width(), f.height()) == (panel.width, panel.height))
            .unwrap_or_else(|| Frame::blank(panel.width, panel.height));
        if let Some(current) = state.current.as_mut() {
            current.draw_over(&mut frame);
        }
        frame
    }
}

/// Shows `frame` on `device`, past the overlay.
fn send(device: &Device, frame: Frame) {
    if let Err(err) = device.presenter.submit(frame) {
        tracing::debug!("cannot show the notification: {err}");
    }
}

/// Draws `notification` for a `width` x `height` frame: the top row of the drawing and its
/// pixels (a band across the bottom third, or the whole frame).
fn render(notification: &Notification, width: u32, height: u32) -> (u32, RgbImage) {
    let band = match notification.style {
        Style::Banner => (height / 3).max(1),
        Style::Full => height,
    };
    let background = notification.color;
    let mut image = RgbImage::from_pixel(width, band, image::Rgb(background));
    let text_color = readable_on(background);
    let detail_color = mix(text_color, background, 0.25);
    let (w, h) = (width as f32, band as f32);
    let style = |px: f32| TextStyle {
        font: builtin_font(),
        px,
        tabular: false,
    };
    let detail = notification.detail.as_deref().filter(|d| !d.is_empty());
    match notification.style {
        Style::Banner => {
            let margin = (h * 0.4).min(w * 0.05);
            let room = w - 2.0 * margin;
            let (title_px, title_line) = if detail.is_some() {
                (h * 0.42, 0.49)
            } else {
                (h * 0.5, 0.5)
            };
            let (title_style, title) = fitted(style(title_px), &notification.text, room);
            let baseline = if detail.is_some() {
                h * title_line
            } else {
                h * title_line + title_style.digit_height() / 2.0
            };
            title_style.draw(&mut image, margin, baseline, &title, text_color);
            if let Some(detail) = detail {
                let (detail_style, detail) = fitted(style(h * 0.24), detail, room);
                detail_style.draw(&mut image, margin, h * 0.84, &detail, detail_color);
            }
        }
        Style::Full => {
            let room = w * 0.92;
            let (title_style, title) = fitted(style(h * 0.26), &notification.text, room);
            let baseline = if detail.is_some() {
                h * 0.5
            } else {
                h * 0.5 + title_style.digit_height() / 2.0
            };
            let x = (w - title_style.width(&title)) / 2.0;
            title_style.draw(&mut image, x, baseline, &title, text_color);
            if let Some(detail) = detail {
                let (detail_style, detail) = fitted(style(h * 0.12), detail, room);
                let x = (w - detail_style.width(&detail)) / 2.0;
                detail_style.draw(&mut image, x, h * 0.76, &detail, detail_color);
            }
        }
    }
    (height - band, image)
}

/// Shrinks `style` (to 70 % at most) until `text` fits in `room`, then shortens the text with
/// an ellipsis.
fn fitted(mut style: TextStyle, text: &str, room: f32) -> (TextStyle, String) {
    let smallest = style.px * 0.7;
    while style.width(text) > room && style.px * 0.95 >= smallest {
        style.px *= 0.95;
    }
    if style.width(text) <= room {
        return (style, text.to_owned());
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let shortened = format!("{}…", chars.iter().collect::<String>().trim_end());
        if style.width(&shortened) <= room {
            return (style, shortened);
        }
    }
    (style, String::new())
}

/// White on dark colors, near-black on light ones.
fn readable_on([r, g, b]: [u8; 3]) -> [u8; 3] {
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if luma > 165.0 {
        [0x11, 0x18, 0x27]
    } else {
        [0xff, 0xff, 0xff]
    }
}

/// `a` moved towards `b` by `amount` (0-1).
fn mix(a: [u8; 3], b: [u8; 3], amount: f32) -> [u8; 3] {
    let channel = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * amount) as u8;
    [channel(0), channel(1), channel(2)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(text: &str) -> Notification {
        Notification {
            text: text.into(),
            detail: None,
            style: Style::Banner,
            color: [0, 0, 200],
            duration: Some(Duration::from_secs(10)),
            wake: false,
        }
    }

    #[test]
    fn checks_texts_and_seconds() {
        assert!(notification("CI failed").validate().is_ok());
        assert!(notification(" ").validate().is_err());
        assert!(notification(&"x".repeat(MAX_TEXT + 1)).validate().is_err());
        let long_detail = Notification {
            detail: Some("x".repeat(MAX_DETAIL + 1)),
            ..notification("a")
        };
        assert!(long_detail.validate().is_err());
        for (duration, ok) in [
            (Some(Duration::ZERO), false),
            (Some(Duration::from_secs(MAX_SECONDS + 1)), false),
            (Some(Duration::from_secs(MAX_SECONDS)), true),
            (None, true),
        ] {
            let n = Notification {
                duration,
                ..notification("a")
            };
            assert_eq!(n.validate().is_ok(), ok, "{duration:?}");
        }
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_color("red"), Some([0xdc, 0x26, 0x26]));
        assert_eq!(parse_color("Blue"), Some([0x25, 0x63, 0xeb]));
        assert_eq!(parse_color("#102030"), Some([0x10, 0x20, 0x30]));
        assert_eq!(parse_color("purple"), None);
    }

    #[test]
    fn draws_a_banner_or_the_whole_panel() {
        let (top, banner) = render(&notification("Claude Code is waiting"), 1920, 462);
        assert_eq!((top, banner.width(), banner.height()), (308, 1920, 154));
        assert_eq!(banner.get_pixel(5, 5).0, [0, 0, 200]);
        let white = banner.pixels().filter(|p| p.0 == [255, 255, 255]).count();
        assert!(white > 500, "the text is drawn in white on blue");

        let full = Notification {
            style: Style::Full,
            color: [0xea, 0xb3, 0x08],
            detail: Some("main · 2 of 41 jobs".into()),
            ..notification("CI failed")
        };
        let (top, image) = render(&full, 1920, 462);
        assert_eq!((top, image.height()), (0, 462));
        let dark = image.pixels().filter(|p| p.0 == [0x11, 0x18, 0x27]).count();
        assert!(dark > 500, "dark text on yellow");
    }

    #[test]
    fn shortens_text_that_does_not_fit() {
        let style = TextStyle {
            font: builtin_font(),
            px: 60.0,
            tabular: false,
        };
        let long = "a very long notification text that cannot fit on the panel at all";
        let (fitted_style, text) = fitted(style, long, 600.0);
        assert!(text.ends_with('…'), "{text}");
        assert!(fitted_style.width(&text) <= 600.0);
        assert!(fitted_style.px >= 60.0 * 0.7 - 0.01);
        let (_, short) = fitted(style, "short", 600.0);
        assert_eq!(short, "short");
    }

    #[test]
    fn passes_frames_and_draws_the_notification_over_them() {
        let overlay = Overlay::new();
        let frame = Frame::blank(1920, 462);
        // Without a notification, frames pass unchanged.
        assert_eq!(overlay.pass(frame.clone()).image(), frame.image());
        overlay.show(notification("hello"));
        let shown = overlay.pass(frame.clone());
        assert_eq!(shown.image().get_pixel(5, 400).0, [0, 0, 200]);
        assert_eq!(
            shown.image().get_pixel(5, 5).0,
            [0, 0, 0],
            "the content shows above"
        );
        assert!(overlay.current().is_some_and(|s| s.left.is_some()));
        assert!(overlay.dismiss());
        assert!(!overlay.dismiss());
        assert_eq!(overlay.pass(frame.clone()).image(), frame.image());
    }

    #[test]
    fn a_notification_ends_on_time_and_only_its_own() {
        let overlay = Overlay::new();
        overlay.show(Notification {
            duration: Some(Duration::from_millis(50)),
            ..notification("first")
        });
        // A newer one is not ended by the first one's timer.
        overlay.show(Notification {
            duration: None,
            ..notification("second")
        });
        std::thread::sleep(Duration::from_millis(150));
        let current = overlay.current().expect("the sticky one stays");
        assert_eq!(current.notification.text, "second");
        assert_eq!(current.left, None);

        overlay.show(Notification {
            duration: Some(Duration::from_millis(50)),
            ..notification("third")
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while overlay.current().is_some() {
            assert!(Instant::now() < deadline, "the notification did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
