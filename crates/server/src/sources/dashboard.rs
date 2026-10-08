//! The built-in dashboard: the time and system figures side by side, with graphs of the last
//! minute.

use std::time::Duration;

use image::RgbImage;
use jiff::Zoned;
use ssp_core::Frame;

use super::Source;
use super::clock::{Clock, until_next_second};
use super::stats::{self, HISTORY, History, Stats};
use crate::config::{ClockConfig, DashboardConfig, Widget, parse_color};
use crate::draw::{self, mix};
use crate::text::{TextStyle, builtin_font};

/// Sizes below are in pixels for a 462-pixel-high panel (the D92) and scale with the height.
const DESIGN_HEIGHT: f32 = 462.0;
const MARGIN: f32 = 22.0;
const GAP: f32 = 18.0;
const RADIUS: f32 = 18.0;
const INSET_X: f32 = 24.0;
const INSET_Y: f32 = 22.0;
const LABEL_PX: f32 = 24.0;
const VALUE_PX: f32 = 84.0;
const DETAIL_PX: f32 = 24.0;
/// The clock panel is this much wider than the others.
const CLOCK_WEIGHT: f32 = 2.0;
/// The network graph never scales below this many bytes per second, so idle noise stays flat.
const MIN_RATE_SCALE: f32 = 100_000.0;

/// A box: x, y, width, height.
type Rect = (f32, f32, f32, f32);

/// The built-in dashboard.
pub struct Dashboard {
    widgets: Vec<Widget>,
    clock: Clock,
    stats: Stats,
    color: [u8; 3],
    accent: [u8; 3],
    background: [u8; 3],
}

impl Dashboard {
    /// Creates a dashboard; its clock panel uses the formats of `clock`. Invalid colors fall back
    /// to the defaults.
    pub fn new(config: DashboardConfig, clock: ClockConfig) -> Self {
        let defaults = DashboardConfig::default();
        let color = |value: &str, default: &str| {
            parse_color(value).unwrap_or_else(|| parse_color(default).unwrap_or([255; 3]))
        };
        Self {
            color: color(&config.color, &defaults.color),
            accent: color(&config.accent, &defaults.accent),
            background: color(&config.background, &defaults.background),
            widgets: config.widgets,
            clock: Clock::new(clock),
            stats: Stats::new(),
        }
    }

    /// Draws the dashboard for `now` with the figures read so far (see [`Stats::refresh`]).
    pub fn draw(&mut self, frame: &mut Frame, now: &Zoned) {
        let image = frame.image_mut();
        image.pixels_mut().for_each(|p| p.0 = self.background);
        let (width, height) = (image.width() as f32, image.height() as f32);
        let s = height / DESIGN_HEIGHT;
        let (margin, gap) = (MARGIN * s, GAP * s);

        let weights: Vec<f32> = self
            .widgets
            .iter()
            .map(|w| {
                if *w == Widget::Clock {
                    CLOCK_WEIGHT
                } else {
                    1.0
                }
            })
            .collect();
        let gaps = gap * self.widgets.len().saturating_sub(1) as f32;
        let unit = (width - 2.0 * margin - gaps) / weights.iter().sum::<f32>();
        let mut x = margin;
        for (widget, weight) in self.widgets.clone().into_iter().zip(weights) {
            let rect = (x, margin, unit * weight, height - 2.0 * margin);
            match widget {
                Widget::Clock => self.draw_clock(image, rect, now),
                other => {
                    let panel = mix(self.background, self.color, 0.07);
                    draw::fill_round_rect(image, rect, RADIUS * s, panel);
                    self.draw_figures(image, rect, s, other);
                }
            }
            x += unit * weight + gap;
        }
    }

    fn draw_clock(&self, image: &mut RgbImage, (x, y, w, h): Rect, now: &Zoned) {
        let time = self.clock.time_text(now);
        let date = self.clock.date_text(now);
        let font = builtin_font();
        let unit = TextStyle {
            font,
            px: 100.0,
            tabular: true,
        };
        let height_share = if date.is_empty() { 0.5 } else { 0.4 };
        let px = (0.92 * w / unit.width(&time).max(1.0))
            .min(height_share * h / unit.digit_height())
            * 100.0;
        let time_style = TextStyle { px, ..unit };
        let date_style = TextStyle {
            px: px * 0.3,
            tabular: false,
            ..unit
        };
        let time_height = time_style.digit_height();
        let (date_height, spacing) = if date.is_empty() {
            (0.0, 0.0)
        } else {
            (date_style.digit_height(), time_height * 0.35)
        };
        let top = y + (h - (time_height + spacing + date_height)) / 2.0;
        let time_x = x + (w - time_style.width(&time)) / 2.0;
        time_style.draw(image, time_x, top + time_height, &time, self.color);
        if !date.is_empty() {
            let date_x = x + (w - date_style.width(&date)) / 2.0;
            let baseline = top + time_height + spacing + date_height;
            let dim = mix(self.background, self.color, 0.7);
            date_style.draw(image, date_x, baseline, &date, dim);
        }
    }

    fn draw_figures(&mut self, image: &mut RgbImage, (x, y, w, h): Rect, s: f32, widget: Widget) {
        let now = &self.stats.now;
        let label_color = mix(self.background, self.color, 0.5);
        let detail_color = mix(self.background, self.color, 0.72);
        let (ix, iy) = (x + INSET_X * s, y + INSET_Y * s);
        let (iw, ih) = (w - 2.0 * INSET_X * s, h - 2.0 * INSET_Y * s);

        let (label, value, unit, detail) = match widget {
            Widget::Cpu => {
                let mut detail = format!("{} cores", now.cpu_count);
                if let Some(load) = now.load {
                    detail.push_str(&format!(" · load {load:.2}"));
                }
                ("CPU", format!("{:.0}", now.cpu_percent), "%".into(), detail)
            }
            Widget::Memory => (
                "MEMORY",
                format!("{:.0}", now.memory_percent()),
                "%".into(),
                format!(
                    "{} / {}",
                    stats::memory(now.memory_used).trim_end_matches(" GB"),
                    stats::memory(now.memory_total)
                ),
            ),
            Widget::Network => {
                let rx = stats::rate(now.rx_per_sec);
                let (value, unit) = rx.split_once(' ').unwrap_or((&rx, ""));
                let detail = stats::rate(now.tx_per_sec);
                ("NETWORK", value.to_owned(), unit.to_owned(), detail)
            }
            Widget::Disk => match now.disk {
                Some((used, total)) => (
                    "DISK",
                    format!("{:.0}", stats::percent(used, total)),
                    "%".into(),
                    format!("{} / {}", stats::bytes(used), stats::bytes(total)),
                ),
                None => ("DISK", "-".into(), String::new(), "not found".into()),
            },
            Widget::Clock => return,
        };

        let font = builtin_font();
        let label_style = TextStyle {
            font,
            px: LABEL_PX * s,
            tabular: false,
        };
        let label_base = iy + label_style.digit_height();
        label_style.draw(image, ix, label_base, label, label_color);

        // A down arrow before the download speed; the upload speed below gets an up arrow.
        let arrow_w = if widget == Widget::Network {
            34.0 * s
        } else {
            0.0
        };
        let value_style = fit(
            TextStyle {
                font,
                px: VALUE_PX * s,
                tabular: true,
            },
            &value,
            &unit,
            iw - arrow_w,
        );
        let unit_style = TextStyle {
            px: value_style.px * 0.42,
            tabular: false,
            ..value_style
        };
        let value_height = value_style.digit_height();
        let value_base = label_base + 22.0 * s + value_height;
        if widget == Widget::Network {
            let size = value_height * 0.5;
            let mark = (ix, value_base - value_height * 0.75, size, size);
            draw::arrow(image, mark, false, self.accent);
        }
        let value_x = ix + arrow_w;
        value_style.draw(image, value_x, value_base, &value, self.color);
        let unit_x = value_x + value_style.width(&value) + 6.0 * s;
        unit_style.draw(image, unit_x, value_base, &unit, detail_color);

        let detail_style = TextStyle {
            font,
            px: DETAIL_PX * s,
            tabular: false,
        };
        let detail_height = detail_style.digit_height();
        let detail_base = value_base + 20.0 * s + detail_height;
        let tx_color = mix(self.background, self.color, 0.6);
        let mut detail_x = ix;
        if widget == Widget::Network {
            let size = detail_height * 0.8;
            let mark = (ix, detail_base - detail_height * 0.9, size, size);
            draw::arrow(image, mark, true, tx_color);
            detail_x += size + 8.0 * s;
        }
        let detail = shorten(detail_style, &detail, ix + iw - detail_x);
        detail_style.draw(image, detail_x, detail_base, &detail, detail_color);

        let graph_y = detail_base + 24.0 * s;
        let graph = (ix, graph_y, iw, iy + ih - graph_y);
        if graph.3 <= 4.0 * s {
            return;
        }
        let accent = self.accent;
        match widget {
            Widget::Cpu => line_graph(image, graph, &mut self.stats.cpu, 100.0, accent),
            Widget::Memory => line_graph(image, graph, &mut self.stats.memory, 100.0, accent),
            Widget::Network => {
                let max = self
                    .stats
                    .rx
                    .max()
                    .max(self.stats.tx.max())
                    .max(MIN_RATE_SCALE);
                line_graph(image, graph, &mut self.stats.rx, max, accent);
                let tx = self.stats.tx.values();
                draw::area_graph(image, graph, tx, HISTORY, max, (tx_color, 0.0));
            }
            Widget::Disk => {
                let used = now_disk_share(&self.stats);
                let track = mix(self.background, self.color, 0.14);
                let bar_h = 16.0 * s;
                let bar = (ix, graph.1 + (graph.3 - bar_h) / 2.0, iw, bar_h);
                draw::fill_round_rect(image, bar, bar_h / 2.0, track);
                if used > 0.0 {
                    let filled = (bar.0, bar.1, (iw * used).max(bar_h), bar_h);
                    draw::fill_round_rect(image, filled, bar_h / 2.0, accent);
                }
            }
            Widget::Clock => {}
        }
    }
}

fn now_disk_share(stats: &Stats) -> f32 {
    stats
        .now
        .disk
        .map_or(0.0, |(used, total)| stats::percent(used, total) / 100.0)
}

fn line_graph(image: &mut RgbImage, rect: Rect, history: &mut History, max: f32, color: [u8; 3]) {
    draw::area_graph(image, rect, history.values(), HISTORY, max, (color, 0.22));
}

/// Shrinks `style` until `value` and its smaller `unit` fit in `width`.
fn fit(style: TextStyle, value: &str, unit: &str, width: f32) -> TextStyle {
    let unit_style = TextStyle {
        px: style.px * 0.42,
        tabular: false,
        ..style
    };
    let needed = style.width(value) + unit_style.width(unit) + style.px * 0.08;
    if needed <= width || needed <= 0.0 {
        style
    } else {
        TextStyle {
            px: style.px * width / needed,
            ..style
        }
    }
}

/// Cuts `text` with an ellipsis so it fits in `width`.
fn shorten(style: TextStyle, text: &str, width: f32) -> String {
    if style.width(text) <= width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if style.width(&candidate) <= width {
            return candidate;
        }
    }
    String::new()
}

impl Source for Dashboard {
    fn render(&mut self, frame: &mut Frame) {
        self.stats.refresh();
        self.draw(frame, &Zoned::now());
    }

    fn next_change(&self) -> Option<Duration> {
        Some(until_next_second())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(frame: &Frame, x: std::ops::Range<u32>) -> usize {
        let image = frame.image();
        x.flat_map(|x| (0..image.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| image.get_pixel(x, y).0 != [0, 0, 0])
            .count()
    }

    #[test]
    fn draws_every_widget() {
        let mut dashboard = Dashboard::new(DashboardConfig::default(), ClockConfig::default());
        dashboard.stats.refresh();
        let mut frame = Frame::blank(1920, 462);
        let now: Zoned = "2026-10-08T12:34:56+09:00[Asia/Tokyo]".parse().unwrap();
        dashboard.draw(&mut frame, &now);
        // The clock fills the left part and each of the four panels has something drawn.
        assert!(lit(&frame, 22..600) > 10_000);
        for start in [650, 960, 1280, 1600] {
            assert!(lit(&frame, start..start + 250) > 10_000, "panel at {start}");
        }
        // The margin stays empty.
        assert_eq!(lit(&frame, 0..20), 0);
    }

    #[test]
    fn fits_other_sizes_and_single_widgets() {
        for (width, height) in [(1280, 400), (800, 480), (480, 1920)] {
            for widgets in [vec![Widget::Network], vec![Widget::Clock, Widget::Disk]] {
                let config = DashboardConfig {
                    widgets,
                    ..DashboardConfig::default()
                };
                let mut dashboard = Dashboard::new(config, ClockConfig::default());
                let mut frame = Frame::blank(width, height);
                dashboard.render(&mut frame);
            }
        }
    }

    #[test]
    fn shortens_long_text() {
        let style = TextStyle {
            font: builtin_font(),
            px: 24.0,
            tabular: false,
        };
        let short = shorten(style, "a rather long line of text", 80.0);
        assert!(short.ends_with('…'));
        assert!(style.width(&short) <= 80.0);
        assert_eq!(shorten(style, "ok", 80.0), "ok");
    }
}
