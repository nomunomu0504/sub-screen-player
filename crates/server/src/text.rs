//! Minimal text drawing on RGB images with an embedded font.
//!
//! The embedded font (Go Medium) covers Latin text. Characters it lacks, such as Japanese,
//! are drawn with a system font found on first use (see [`fallback_font`]).

use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, point};
use image::RgbImage;

static GO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Go-Medium.ttf");

/// System fonts tried, in order, for characters the built-in font lacks. They cover Japanese
/// (and most Chinese and Korean) text on macOS, Windows and common Linux distributions.
const FALLBACK_FAMILIES: &[&str] = &[
    // macOS
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    // Windows
    "Yu Gothic UI",
    "Yu Gothic",
    "Meiryo",
    // Linux
    "Noto Sans CJK JP",
    "Noto Sans JP",
    "Source Han Sans JP",
    "IPAexGothic",
    "IPAGothic",
    "VL Gothic",
];

/// The built-in font (Go Medium, see `assets/fonts/LICENSE-Go-fonts.txt`).
pub fn builtin_font() -> &'static FontArc {
    static FONT: OnceLock<FontArc> = OnceLock::new();
    FONT.get_or_init(|| FontArc::try_from_slice(GO_MEDIUM).expect("embedded font is valid"))
}

/// The system font used for characters the built-in font lacks, if one is installed.
///
/// The system's fonts are searched the first time this is called, which can take a moment, so
/// it is only called when such a character is drawn.
pub fn fallback_font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(load_fallback).as_ref()
}

fn load_fallback() -> Option<FontArc> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for family in FALLBACK_FAMILIES {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: fontdb::Weight::MEDIUM,
            ..fontdb::Query::default()
        };
        let Some(id) = db.query(&query) else {
            continue;
        };
        let font = db.with_face_data(id, |data, index| {
            FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
        });
        if let Some(font) = font.flatten() {
            tracing::debug!(family, "using fallback font");
            return Some(FontArc::new(font));
        }
    }
    tracing::warn!("no system font for non-Latin text found; such characters are not drawn");
    None
}

/// The font that draws `c`: the built-in one if it has the character, else the fallback.
fn font_for(primary: &'static FontArc, c: char) -> &'static FontArc {
    if c.is_ascii() || c.is_whitespace() || primary.glyph_id(c) != GlyphId(0) {
        return primary;
    }
    match fallback_font() {
        Some(fallback) if fallback.glyph_id(c) != GlyphId(0) => fallback,
        _ => primary,
    }
}

/// A font at a size. With `tabular`, all digits take the same width so numbers do not jitter.
#[derive(Clone, Copy)]
pub struct TextStyle {
    /// The font. Characters it lacks are drawn with [`fallback_font`].
    pub font: &'static FontArc,
    /// Size in pixels.
    pub px: f32,
    /// Give every digit the width of the widest one.
    pub tabular: bool,
}

impl TextStyle {
    fn digit_advance(&self) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        ('0'..='9')
            .map(|c| scaled.h_advance(self.font.glyph_id(c)))
            .fold(0.0, f32::max)
    }

    /// Calls `place` with each glyph, its font and its pen x position; returns the total width.
    fn layout(&self, text: &str, mut place: impl FnMut(&FontArc, GlyphId, f32)) -> f32 {
        let scale = PxScale::from(self.px);
        let digit = self.tabular.then(|| self.digit_advance());
        let mut x = 0.0;
        let mut prev: Option<(GlyphId, &FontArc)> = None;
        for c in text.chars() {
            let font = font_for(self.font, c);
            let scaled = font.as_scaled(scale);
            let id = font.glyph_id(c);
            let advance = scaled.h_advance(id);
            match digit.filter(|_| c.is_ascii_digit()) {
                Some(cell) => {
                    place(font, id, x + (cell - advance) / 2.0);
                    x += cell;
                    prev = None;
                }
                None => {
                    // Kerning only applies between glyphs of the same font.
                    if let Some((prev, prev_font)) = prev
                        && std::ptr::eq(prev_font, font)
                    {
                        x += scaled.kern(prev, id);
                    }
                    place(font, id, x);
                    x += advance;
                    prev = Some((id, font));
                }
            }
        }
        x
    }

    /// Width of `text` in pixels.
    pub fn width(&self, text: &str) -> f32 {
        self.layout(text, |_, _, _| {})
    }

    /// Height of digits above the baseline, in pixels.
    pub fn digit_height(&self) -> f32 {
        let glyph = self.font.glyph_id('0').with_scale(self.px);
        self.font
            .outline_glyph(glyph)
            .map_or(self.px * 0.7, |g| -g.px_bounds().min.y)
    }

    /// Draws `text` with its left end at `x` and its baseline at `baseline`.
    pub fn draw(&self, image: &mut RgbImage, x: f32, baseline: f32, text: &str, color: [u8; 3]) {
        let (w, h) = (image.width() as i32, image.height() as i32);
        self.layout(text, |font, id, pen| {
            let glyph = id.with_scale_and_position(self.px, point(x + pen, baseline));
            let Some(outline) = font.outline_glyph(glyph) else {
                return;
            };
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if (0..w).contains(&px) && (0..h).contains(&py) {
                    let dst = image.get_pixel_mut(px as u32, py as u32);
                    let a = coverage.clamp(0.0, 1.0);
                    for (d, s) in dst.0.iter_mut().zip(color) {
                        *d = (f32::from(*d) * (1.0 - a) + f32::from(s) * a).round() as u8;
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabular_digits_have_equal_width() {
        let style = TextStyle {
            font: builtin_font(),
            px: 100.0,
            tabular: true,
        };
        assert_eq!(style.width("11:11"), style.width("00:00"));
    }

    #[test]
    fn draws_inside_the_image() {
        let style = TextStyle {
            font: builtin_font(),
            px: 40.0,
            tabular: false,
        };
        let mut image = RgbImage::new(200, 60);
        style.draw(&mut image, 5.0, 45.0, "Hi 42", [255, 255, 255]);
        assert!(image.pixels().any(|p| p.0 == [255, 255, 255]));
        // Drawing partly outside must not panic.
        style.draw(&mut image, -30.0, 10.0, "edge", [255, 0, 0]);
    }

    #[test]
    fn draws_japanese_with_a_system_font_when_there_is_one() {
        let style = TextStyle {
            font: builtin_font(),
            px: 40.0,
            tabular: false,
        };
        let mut image = RgbImage::new(200, 60);
        // Never panics, with or without a fallback font.
        style.draw(&mut image, 5.0, 45.0, "10月8日(木)", [255, 255, 255]);
        if fallback_font().is_some() {
            let mut kanji = RgbImage::new(60, 60);
            style.draw(&mut kanji, 5.0, 45.0, "木", [255, 255, 255]);
            assert!(kanji.pixels().any(|p| p.0 != [0, 0, 0]));
        }
    }
}
