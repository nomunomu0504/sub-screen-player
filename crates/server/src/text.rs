//! Minimal text drawing on RGB images with an embedded font.

use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont, point};
use image::RgbImage;

static GO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Go-Medium.ttf");

/// The built-in font (Go Medium, see `assets/fonts/LICENSE-Go-fonts.txt`).
pub fn builtin_font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(GO_MEDIUM).expect("embedded font is valid"))
}

/// A font at a size. With `tabular`, all digits take the same width so numbers do not jitter.
#[derive(Clone, Copy)]
pub struct TextStyle<'a> {
    /// The font.
    pub font: &'a FontRef<'a>,
    /// Size in pixels.
    pub px: f32,
    /// Give every digit the width of the widest one.
    pub tabular: bool,
}

impl TextStyle<'_> {
    fn digit_advance(&self) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        ('0'..='9')
            .map(|c| scaled.h_advance(self.font.glyph_id(c)))
            .fold(0.0, f32::max)
    }

    /// Calls `place` with each glyph and its pen x position; returns the total width.
    fn layout(&self, text: &str, mut place: impl FnMut(GlyphId, f32)) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(self.px));
        let digit = self.tabular.then(|| self.digit_advance());
        let mut x = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in text.chars() {
            let id = self.font.glyph_id(c);
            let advance = scaled.h_advance(id);
            match digit.filter(|_| c.is_ascii_digit()) {
                Some(cell) => {
                    place(id, x + (cell - advance) / 2.0);
                    x += cell;
                    prev = None;
                }
                None => {
                    if let Some(prev) = prev {
                        x += scaled.kern(prev, id);
                    }
                    place(id, x);
                    x += advance;
                    prev = Some(id);
                }
            }
        }
        x
    }

    /// Width of `text` in pixels.
    pub fn width(&self, text: &str) -> f32 {
        self.layout(text, |_, _| {})
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
        self.layout(text, |id, pen| {
            let glyph = id.with_scale_and_position(self.px, point(x + pen, baseline));
            let Some(outline) = self.font.outline_glyph(glyph) else {
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
}
