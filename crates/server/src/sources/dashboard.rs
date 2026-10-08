//! The built-in dashboard: the time and system figures side by side, with graphs of the last
//! minute.

use std::time::Duration;

use image::RgbImage;
use jiff::Zoned;
use ssp_core::Frame;

use super::Source;
use super::clock::{Clock, until_next_second};
use super::stats::{self, HISTORY, Stats};
use crate::claude_code;
use crate::config::{ClockConfig, DashboardConfig, Widget, parse_color};
use crate::draw::{self, mix};
use crate::metrics::{Metric, Metrics};
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

/// What one figure panel shows.
struct Panel {
    label: String,
    value: String,
    unit: String,
    detail: String,
    /// Waiting for data, or stale: drawn dimmed.
    dim: bool,
    graph: Graph,
}

/// The lower part of a panel.
enum Graph {
    None,
    /// Recent values, oldest first, scaled so `max` is the top. The newest is at the right edge
    /// and the graph is `slots` values wide.
    Line {
        values: Vec<f32>,
        max: f32,
        slots: usize,
    },
    /// Download (accent) and upload (line) speeds, with arrows next to the figures.
    Network {
        rx: Vec<f32>,
        tx: Vec<f32>,
        max: f32,
    },
    /// A bar filled to a share (0-1).
    Bar(f32),
}

/// The built-in dashboard.
pub struct Dashboard {
    widgets: Vec<Widget>,
    clock: Clock,
    stats: Stats,
    metrics: Metrics,
    color: [u8; 3],
    accent: [u8; 3],
    background: [u8; 3],
}

impl Dashboard {
    /// Creates a dashboard; its clock panel uses the formats of `clock` and `metric:<id>`
    /// panels read `metrics`. Invalid colors fall back to the defaults.
    pub fn new(config: DashboardConfig, clock: ClockConfig, metrics: Metrics) -> Self {
        if config.widgets.contains(&Widget::ClaudeCode) {
            metrics.activate(claude_code::METRIC_ID);
        }
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
            metrics,
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
        for (widget, weight) in self.widgets.iter().zip(weights) {
            let rect = (x, margin, unit * weight, height - 2.0 * margin);
            match widget {
                Widget::Clock => self.draw_clock(image, rect, now),
                other => {
                    let panel = mix(self.background, self.color, 0.07);
                    draw::fill_round_rect(image, rect, RADIUS * s, panel);
                    let figures = self.panel(other);
                    self.draw_panel(image, rect, s, &figures);
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

    /// The figures of a panel.
    fn panel(&self, widget: &Widget) -> Panel {
        let now = &self.stats.now;
        let built_in = |label: &str, value: String, unit: &str, detail: String, graph| Panel {
            label: label.to_owned(),
            value,
            unit: unit.to_owned(),
            detail,
            dim: false,
            graph,
        };
        match widget {
            Widget::Cpu => {
                let mut detail = format!("{} cores", now.cpu_count);
                if let Some(load) = now.load {
                    detail.push_str(&format!(" · load {load:.2}"));
                }
                let graph = Graph::Line {
                    values: self.stats.cpu.values(),
                    max: 100.0,
                    slots: HISTORY,
                };
                built_in("CPU", format!("{:.0}", now.cpu_percent), "%", detail, graph)
            }
            Widget::Memory => {
                let detail = format!(
                    "{} / {}",
                    stats::memory(now.memory_used).trim_end_matches(" GB"),
                    stats::memory(now.memory_total)
                );
                let graph = Graph::Line {
                    values: self.stats.memory.values(),
                    max: 100.0,
                    slots: HISTORY,
                };
                built_in(
                    "MEMORY",
                    format!("{:.0}", now.memory_percent()),
                    "%",
                    detail,
                    graph,
                )
            }
            Widget::Network => {
                let rx = stats::rate(now.rx_per_sec);
                let (value, unit) = rx.split_once(' ').unwrap_or((&rx, ""));
                let graph = Graph::Network {
                    rx: self.stats.rx.values(),
                    tx: self.stats.tx.values(),
                    max: self
                        .stats
                        .rx
                        .max()
                        .max(self.stats.tx.max())
                        .max(MIN_RATE_SCALE),
                };
                let detail = stats::rate(now.tx_per_sec);
                built_in("NETWORK", value.to_owned(), unit, detail, graph)
            }
            Widget::Disk => match now.disk {
                Some((used, total)) => built_in(
                    "DISK",
                    format!("{:.0}", stats::percent(used, total)),
                    "%",
                    format!("{} / {}", stats::bytes(used), stats::bytes(total)),
                    Graph::Bar(stats::percent(used, total) / 100.0),
                ),
                None => built_in("DISK", "-".into(), "", "not found".into(), Graph::None),
            },
            Widget::Metric(id) => self.metric(id),
            // Its figures are a metric kept up to date by `claude_code`.
            Widget::ClaudeCode => self.metric(claude_code::METRIC_ID),
            Widget::Clock => unreachable!("the clock is not a figure panel"),
        }
    }

    fn draw_panel(&self, image: &mut RgbImage, (x, y, w, h): Rect, s: f32, panel: &Panel) {
        let label_color = mix(self.background, self.color, 0.5);
        let detail_color = mix(self.background, self.color, 0.72);
        let value_color = if panel.dim {
            mix(self.background, self.color, 0.45)
        } else {
            self.color
        };
        let network = matches!(panel.graph, Graph::Network { .. });
        let (ix, iy) = (x + INSET_X * s, y + INSET_Y * s);
        let (iw, ih) = (w - 2.0 * INSET_X * s, h - 2.0 * INSET_Y * s);

        let font = builtin_font();
        let label_style = TextStyle {
            font,
            px: LABEL_PX * s,
            tabular: false,
        };
        let label_base = iy + label_style.digit_height();
        let label = shorten(label_style, &panel.label, iw);
        label_style.draw(image, ix, label_base, &label, label_color);

        // A down arrow before the download speed; the upload speed below gets an up arrow.
        let arrow_w = if network { 34.0 * s } else { 0.0 };
        let value_style = fit(
            TextStyle {
                font,
                px: VALUE_PX * s,
                tabular: true,
            },
            &panel.value,
            &panel.unit,
            iw - arrow_w,
        );
        let unit_style = TextStyle {
            px: value_style.px * 0.42,
            tabular: false,
            ..value_style
        };
        let value_height = value_style.digit_height();
        let value_base = label_base + 22.0 * s + value_height;
        if network {
            let size = value_height * 0.5;
            let mark = (ix, value_base - value_height * 0.75, size, size);
            draw::arrow(image, mark, false, self.accent);
        }
        let value_x = ix + arrow_w;
        value_style.draw(image, value_x, value_base, &panel.value, value_color);
        let unit_x = value_x + value_style.width(&panel.value) + 6.0 * s;
        unit_style.draw(image, unit_x, value_base, &panel.unit, detail_color);

        let detail_style = TextStyle {
            font,
            px: DETAIL_PX * s,
            tabular: false,
        };
        let detail_height = detail_style.digit_height();
        let detail_base = value_base + 20.0 * s + detail_height;
        let tx_color = mix(self.background, self.color, 0.6);
        let mut detail_x = ix;
        if network {
            let size = detail_height * 0.8;
            let mark = (ix, detail_base - detail_height * 0.9, size, size);
            draw::arrow(image, mark, true, tx_color);
            detail_x += size + 8.0 * s;
        }
        let detail = shorten(detail_style, &panel.detail, ix + iw - detail_x);
        detail_style.draw(image, detail_x, detail_base, &detail, detail_color);

        let graph_y = detail_base + 24.0 * s;
        let area = (ix, graph_y, iw, iy + ih - graph_y);
        if area.3 <= 4.0 * s {
            return;
        }
        let accent = if panel.dim {
            mix(self.background, self.accent, 0.45)
        } else {
            self.accent
        };
        match &panel.graph {
            Graph::None => {}
            Graph::Line { values, max, slots } => {
                draw::area_graph(image, area, values, *slots, *max, (accent, 0.22));
            }
            Graph::Network { rx, tx, max } => {
                draw::area_graph(image, area, rx, HISTORY, *max, (accent, 0.22));
                draw::area_graph(image, area, tx, HISTORY, *max, (tx_color, 0.0));
            }
            Graph::Bar(share) => {
                let track = mix(self.background, self.color, 0.14);
                let bar_h = 16.0 * s;
                let bar = (ix, area.1 + (area.3 - bar_h) / 2.0, iw, bar_h);
                draw::fill_round_rect(image, bar, bar_h / 2.0, track);
                if *share > 0.0 {
                    let filled = (bar.0, bar.1, (iw * share.min(1.0)).max(bar_h), bar_h);
                    draw::fill_round_rect(image, filled, bar_h / 2.0, accent);
                }
            }
        }
    }
}

impl Dashboard {
    /// The panel of metric `id`, or a placeholder until it arrives.
    fn metric(&self, id: &str) -> Panel {
        match self.metrics.get(id) {
            Some(metric) => metric_panel(id, &metric),
            None => Panel {
                label: id.to_owned(),
                value: "-".into(),
                unit: String::new(),
                detail: "waiting for data".into(),
                dim: true,
                graph: Graph::None,
            },
        }
    }
}

/// The panel of a metric sent from outside.
fn metric_panel(id: &str, metric: &Metric) -> Panel {
    let stale = metric.is_stale();
    let (value, unit) = match (&metric.text, metric.value) {
        (Some(text), _) => (text.clone(), String::new()),
        (None, Some(value)) => (number(value), metric.unit.clone()),
        (None, None) => ("-".into(), String::new()),
    };
    let mut detail = metric.detail.clone();
    if stale {
        let ago = ago(metric.age());
        detail = if detail.is_empty() {
            ago
        } else {
            format!("{detail} · {ago}")
        };
    }
    let graph = if metric.text.is_none() && metric.history.len() >= 2 {
        let values: Vec<f32> = metric.history.iter().copied().collect();
        let max = metric
            .max
            .map_or_else(|| values.iter().copied().fold(0.0, f32::max), |m| m as f32);
        // Values come at their sender's pace, so the ones there are fill the width.
        Graph::Line {
            slots: values.len(),
            values,
            max: max.max(f32::MIN_POSITIVE),
        }
    } else {
        Graph::None
    };
    Panel {
        label: metric.label.clone().unwrap_or_else(|| id.to_owned()),
        value,
        unit,
        detail,
        dim: stale,
        graph,
    }
}

/// Formats a metric value: whole numbers as they are, others with about three significant digits.
/// From 100,000 on, numbers are shortened with k, M or B (`123k`, `4.56M`).
pub(crate) fn number(value: f64) -> String {
    let (value, suffix) = match value.abs() {
        v if v >= 999.5e6 => (value / 1e9, "B"),
        v if v >= 999.5e3 => (value / 1e6, "M"),
        v if v >= 1e5 => (value / 1e3, "k"),
        _ => (value, ""),
    };
    let text = if (suffix.is_empty() && value.fract() == 0.0) || value.abs() >= 100.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    };
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    format!("{text}{suffix}")
}

/// "45 s ago", "12 min ago", "3 h ago".
fn ago(age: Duration) -> String {
    match age.as_secs() {
        s if s < 120 => format!("{s} s ago"),
        s if s < 2 * 3600 => format!("{} min ago", s / 60),
        s => format!("{} h ago", s / 3600),
    }
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
    use std::time::Instant;

    use super::*;
    use crate::metrics::MetricUpdate;

    fn lit(frame: &Frame, x: std::ops::Range<u32>) -> usize {
        let image = frame.image();
        x.flat_map(|x| (0..image.height()).map(move |y| (x, y)))
            .filter(|&(x, y)| image.get_pixel(x, y).0 != [0, 0, 0])
            .count()
    }

    #[test]
    fn draws_every_widget() {
        let mut dashboard = Dashboard::new(
            DashboardConfig::default(),
            ClockConfig::default(),
            Metrics::default(),
        );
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
                let mut dashboard =
                    Dashboard::new(config, ClockConfig::default(), Metrics::default());
                let mut frame = Frame::blank(width, height);
                dashboard.render(&mut frame);
            }
        }
    }

    #[test]
    fn metric_panels_show_what_was_sent() {
        let metrics = Metrics::default();
        let dashboard = Dashboard::new(
            DashboardConfig::default(),
            ClockConfig::default(),
            metrics.clone(),
        );
        let ci = Widget::Metric("ci".into());
        let waiting = dashboard.panel(&ci);
        assert_eq!(
            (waiting.detail.as_str(), waiting.dim),
            ("waiting for data", true)
        );

        let update = MetricUpdate {
            label: Some("CI".into()),
            unit: Some("failed".into()),
            series: Some(vec![0.0, 4.0, 2.5]),
            ..MetricUpdate::default()
        };
        metrics.set("ci", update).unwrap();
        let panel = dashboard.panel(&ci);
        assert_eq!((panel.label.as_str(), panel.value.as_str()), ("CI", "2.5"));
        assert_eq!(panel.unit, "failed");
        assert!(!panel.dim);
        let Graph::Line { values, max, slots } = panel.graph else {
            panic!("a metric with values has a graph");
        };
        assert_eq!((values.len(), slots, max), (3, 3, 4.0));

        let text = MetricUpdate {
            text: Some("passing".into()),
            ..MetricUpdate::default()
        };
        metrics.set("ci", text).unwrap();
        let panel = dashboard.panel(&ci);
        assert_eq!(panel.value, "passing");
        assert!(matches!(panel.graph, Graph::None));
    }

    #[test]
    fn old_metric_values_are_dimmed() {
        let metrics = Metrics::default();
        let update = MetricUpdate {
            value: Some(1.0),
            detail: Some("main".into()),
            ..MetricUpdate::default()
        };
        metrics.set("ci", update).unwrap();
        let mut metric = metrics.get("ci").unwrap();
        let Some(earlier) = Instant::now().checked_sub(Duration::from_secs(600)) else {
            return; // The clock started less than ten minutes ago.
        };
        metric.updated = earlier;
        let panel = metric_panel("ci", &metric);
        assert!(panel.dim);
        assert_eq!(panel.detail, "main · 10 min ago");
    }

    #[test]
    fn formats_metric_numbers() {
        assert_eq!(number(3.0), "3");
        assert_eq!(number(99_999.0), "99999");
        assert_eq!(number(123_456.0), "123k");
        assert_eq!(number(999_999.0), "1M");
        assert_eq!(number(1_234_567.0), "1.23M");
        assert_eq!(number(-45_600_000.0), "-45.6M");
        assert_eq!(number(7.8e9), "7.8B");
        assert_eq!(number(42.26), "42.3");
        assert_eq!(number(0.126), "0.13");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(250.4), "250");
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
