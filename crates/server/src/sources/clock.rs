use std::time::Duration;

use jiff::{Timestamp, Zoned, fmt::strtime};
use ssp_core::Frame;

use super::Source;
use crate::config::{ClockConfig, parse_color};
use crate::text::{TextStyle, builtin_font};

/// Share of the panel width the time may use.
const MAX_WIDTH: f32 = 0.9;
/// Date size relative to the time.
const DATE_SCALE: f32 = 0.24;

/// The built-in clock: the time in large digits with an optional date line below.
pub struct Clock {
    config: ClockConfig,
    color: [u8; 3],
    background: [u8; 3],
}

impl Clock {
    /// Creates a clock. Invalid colors fall back to white on black.
    pub fn new(config: ClockConfig) -> Self {
        let color = parse_color(&config.color).unwrap_or([255; 3]);
        let background = parse_color(&config.background).unwrap_or([0; 3]);
        Self {
            config,
            color,
            background,
        }
    }

    /// Checks that the formats can be used, so errors surface before anything is drawn.
    pub fn validate(config: &ClockConfig) -> Result<(), String> {
        if !matches!(config.weekdays.len(), 0 | 7) {
            return Err("weekdays needs 7 names, from Sunday".into());
        }
        let now = Zoned::now();
        for format in [config.time_format(), config.date_format.as_str()] {
            format_time(format, &now, &config.weekdays)
                .map_err(|e| format!("bad format {format:?}: {e}"))?;
        }
        Ok(())
    }

    /// The big line (the time) for `now`.
    pub fn time_text(&self, now: &Zoned) -> String {
        format_time(self.config.time_format(), now, &self.config.weekdays)
            .unwrap_or_else(|e| e.to_string())
    }

    /// The small line (the date) for `now`; empty if hidden.
    pub fn date_text(&self, now: &Zoned) -> String {
        format_time(&self.config.date_format, now, &self.config.weekdays).unwrap_or_default()
    }

    /// Draws the clock for `now`.
    pub fn draw(&self, frame: &mut Frame, now: &Zoned) {
        let image = frame.image_mut();
        image.pixels_mut().for_each(|p| p.0 = self.background);
        let (width, height) = (image.width() as f32, image.height() as f32);
        let time = self.time_text(now);
        let date = self.date_text(now);

        // Size the time to the panel, leaving room for the date.
        let font = builtin_font();
        let unit = TextStyle {
            font,
            px: 100.0,
            tabular: true,
        };
        let height_share = if date.is_empty() { 0.62 } else { 0.5 };
        let px = (MAX_WIDTH * width / unit.width(&time).max(1.0))
            .min(height_share * height / unit.digit_height())
            * 100.0;
        let time_style = TextStyle {
            font,
            px,
            tabular: true,
        };
        let date_style = TextStyle {
            font,
            px: px * DATE_SCALE,
            tabular: true,
        };

        let time_height = time_style.digit_height();
        let (date_height, gap) = if date.is_empty() {
            (0.0, 0.0)
        } else {
            (date_style.digit_height(), time_height * 0.3)
        };
        let top = (height - (time_height + gap + date_height)) / 2.0;

        let time_x = (width - time_style.width(&time)) / 2.0;
        time_style.draw(image, time_x, top + time_height, &time, self.color);
        if !date.is_empty() {
            let date_x = (width - date_style.width(&date)) / 2.0;
            let dim = self.color.map(|c| (u16::from(c) * 3 / 4) as u8);
            date_style.draw(
                image,
                date_x,
                top + time_height + gap + date_height,
                &date,
                dim,
            );
        }
    }
}

/// Formats `now` with a strftime-style `format`. With 7 `weekdays` (from Sunday), `%a` and `%A`
/// are replaced by the day's name, e.g. for Japanese.
pub fn format_time(format: &str, now: &Zoned, weekdays: &[String]) -> Result<String, jiff::Error> {
    let [_, _, _, _, _, _, _] = weekdays else {
        return strtime::format(format, now);
    };
    let name = &weekdays[now.weekday().to_sunday_zero_offset() as usize];
    let mut localized = String::with_capacity(format.len());
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            localized.push(c);
            continue;
        }
        match chars.next() {
            Some('a' | 'A') => localized.push_str(&name.replace('%', "%%")),
            Some(next) => {
                localized.push('%');
                localized.push(next);
            }
            None => localized.push('%'),
        }
    }
    strtime::format(localized.as_str(), now)
}

impl Source for Clock {
    fn render(&mut self, frame: &mut Frame) {
        self.draw(frame, &Zoned::now());
    }

    fn next_change(&self) -> Option<Duration> {
        // Wake just after the next full second; unchanged frames are skipped downstream.
        let into_second = Timestamp::now().subsec_nanosecond().max(0) as u64;
        Some(Duration::from_nanos(1_000_000_000 - into_second) + Duration::from_millis(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Zoned {
        text.parse().unwrap()
    }

    #[test]
    fn draws_time_and_date() {
        let clock = Clock::new(ClockConfig::default());
        let mut frame = Frame::blank(1920, 462);
        clock.draw(&mut frame, &at("2026-10-07T12:34:56+09:00[Asia/Tokyo]"));
        let lit = frame.image().pixels().filter(|p| p.0 != [0, 0, 0]).count();
        assert!(lit > 10_000, "only {lit} pixels drawn");
        // Text stays clear of the panel edges.
        let image = frame.image();
        assert!((0..462).all(|y| image.get_pixel(0, y).0 == [0, 0, 0]));
        assert!((0..1920).all(|x| image.get_pixel(x, 0).0 == [0, 0, 0]));
    }

    #[test]
    fn rejects_bad_formats() {
        let config = ClockConfig {
            date_format: "%Y %".into(),
            ..ClockConfig::default()
        };
        assert!(Clock::validate(&config).is_err());
        assert!(Clock::validate(&ClockConfig::default()).is_ok());
    }

    #[test]
    fn uses_weekday_names() {
        let weekdays: Vec<String> = ["日", "月", "火", "水", "木", "金", "土"]
            .map(String::from)
            .into();
        // 2026-10-08 is a Thursday.
        let now = at("2026-10-08T09:00:00+09:00[Asia/Tokyo]");
        assert_eq!(
            format_time("%m月%d日(%a) %A 100%%", &now, &weekdays).unwrap(),
            "10月08日(木) 木 100%"
        );
        assert_eq!(format_time("%a", &now, &[]).unwrap(), "Thu");
        let config = ClockConfig {
            weekdays: vec!["x".into()],
            ..ClockConfig::default()
        };
        assert!(Clock::validate(&config).is_err());
    }

    #[test]
    fn wakes_within_a_second() {
        let wait = Clock::new(ClockConfig::default()).next_change().unwrap();
        assert!(wait <= Duration::from_millis(1002));
    }
}
