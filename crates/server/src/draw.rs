//! Small drawing helpers on RGB images: blended pixels, rounded panels, graphs and arrows.
//!
//! Coordinates are in pixels as `f32`; shapes are clipped to the image and their edges are
//! anti-aliased where it shows (rounded corners, graph lines).

use image::RgbImage;

/// Mixes `a` and `b`: `t = 0` gives `a`, `t = 1` gives `b`.
pub fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    std::array::from_fn(|i| (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t).round() as u8)
}

/// Blends `color` over the pixel at (`x`, `y`) with opacity `alpha`; outside pixels are ignored.
pub fn blend(image: &mut RgbImage, x: i64, y: i64, color: [u8; 3], alpha: f32) {
    if alpha <= 0.0 || x < 0 || y < 0 || x >= i64::from(image.width()) {
        return;
    }
    if y >= i64::from(image.height()) {
        return;
    }
    let pixel = image.get_pixel_mut(x as u32, y as u32);
    pixel.0 = mix(pixel.0, color, alpha);
}

/// Fills a rectangle with rounded corners of radius `r`.
pub fn fill_round_rect(
    image: &mut RgbImage,
    (x, y, w, h): (f32, f32, f32, f32),
    r: f32,
    color: [u8; 3],
) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    for py in y.floor() as i64..(y + h).ceil() as i64 {
        for px in x.floor() as i64..(x + w).ceil() as i64 {
            // Distance from the pixel center into the nearest corner circle decides coverage.
            let (cx, cy) = (px as f32 + 0.5, py as f32 + 0.5);
            let dx = (x + r - cx).max(cx - (x + w - r)).max(0.0);
            let dy = (y + r - cy).max(cy - (y + h - r)).max(0.0);
            let outside = (dx * dx + dy * dy).sqrt() - r;
            let edge_x = (cx - x).min(x + w - cx) + 0.5;
            let edge_y = (cy - y).min(y + h - cy) + 0.5;
            let coverage =
                (0.5 - outside).clamp(0.0, 1.0) * edge_x.clamp(0.0, 1.0) * edge_y.clamp(0.0, 1.0);
            blend(image, px, py, color, coverage);
        }
    }
}

/// Draws `samples` (oldest first, newest at the right edge) as a line with the area below it
/// filled at opacity `fill` (0 draws only the line), scaled so that `max` reaches the top of the
/// box. `slots` is how many samples fit the width; fewer samples leave the left part empty.
pub fn area_graph(
    image: &mut RgbImage,
    (x, y, w, h): (f32, f32, f32, f32),
    samples: &[f32],
    slots: usize,
    max: f32,
    (color, fill): ([u8; 3], f32),
) {
    if samples.is_empty() || slots < 2 || max <= 0.0 {
        return;
    }
    let step = w / (slots - 1) as f32;
    let start = x + w - step * (samples.len() - 1) as f32;
    let value_at = |px: f32| -> Option<f32> {
        let pos = (px - start) / step;
        if pos < 0.0 {
            return None;
        }
        let i = (pos.floor() as usize).min(samples.len() - 1);
        let next = samples.get(i + 1).copied().unwrap_or(samples[i]);
        let t = pos - i as f32;
        Some(samples[i] * (1.0 - t) + next * t)
    };
    let top_at = |px: f32| value_at(px).map(|v| y + h - (v / max).clamp(0.0, 1.0) * h);
    let half = (h * 0.01).clamp(0.75, 2.0);
    for px in x.floor() as i64..(x + w).ceil() as i64 {
        // The curve's height at both edges of the column: a steep part becomes a vertical run
        // of line pixels, so the line has no gaps.
        let (Some(left), Some(right)) = (top_at(px as f32), top_at(px as f32 + 1.0)) else {
            continue;
        };
        let (line_top, line_bottom) = (left.min(right) - half, left.max(right) + half);
        let middle = (left + right) / 2.0;
        for py in line_top.floor() as i64..(y + h).ceil() as i64 {
            let cy = py as f32 + 0.5;
            let in_line =
                (cy - line_top + 0.5).clamp(0.0, 1.0) * (line_bottom - cy + 0.5).clamp(0.0, 1.0);
            let in_area = (cy - middle + 0.5).clamp(0.0, 1.0);
            blend(image, px, py, color, in_line.max(fill * in_area));
        }
    }
}

/// Fills a triangle pointing up (`up = true`) or down inside the given box: an arrow mark.
pub fn arrow(image: &mut RgbImage, (x, y, w, h): (f32, f32, f32, f32), up: bool, color: [u8; 3]) {
    for py in y.floor() as i64..(y + h).ceil() as i64 {
        // Fraction of the way from the tip to the base at this row.
        let row = (py as f32 + 0.5 - y) / h;
        let t = if up { row } else { 1.0 - row };
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        let half = w / 2.0 * t;
        let (left, right) = (x + w / 2.0 - half, x + w / 2.0 + half);
        for px in left.floor() as i64..right.ceil() as i64 {
            let cx = px as f32 + 0.5;
            let coverage = (cx - left + 0.5).clamp(0.0, 1.0) * (right - cx + 0.5).clamp(0.0, 1.0);
            blend(image, px, py, color, coverage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 3] = [255, 255, 255];

    #[test]
    fn mixes_colors() {
        assert_eq!(mix([0, 0, 0], [200, 100, 50], 0.5), [100, 50, 25]);
        assert_eq!(mix([1, 2, 3], WHITE, 0.0), [1, 2, 3]);
    }

    #[test]
    fn rounded_rect_fills_the_middle_and_spares_the_corner() {
        let mut image = RgbImage::new(100, 60);
        fill_round_rect(&mut image, (10.0, 10.0, 80.0, 40.0), 12.0, WHITE);
        assert_eq!(image.get_pixel(50, 30).0, WHITE);
        assert_eq!(image.get_pixel(10, 10).0, [0, 0, 0]);
        assert_eq!(image.get_pixel(5, 30).0, [0, 0, 0]);
    }

    #[test]
    fn graph_puts_the_newest_sample_at_the_right() {
        let mut image = RgbImage::new(100, 50);
        let graph = (0.0, 0.0, 100.0, 50.0);
        area_graph(&mut image, graph, &[0.0, 100.0], 10, 100.0, (WHITE, 0.2));
        // The right edge is at full height, the left part has no data yet.
        assert!(image.get_pixel(99, 1).0[0] > 200);
        assert_eq!(image.get_pixel(10, 49).0, [0, 0, 0]);
    }

    #[test]
    fn shapes_outside_the_image_do_not_panic() {
        let mut image = RgbImage::new(10, 10);
        fill_round_rect(&mut image, (-5.0, -5.0, 30.0, 30.0), 4.0, WHITE);
        arrow(&mut image, (5.0, 5.0, 20.0, 20.0), true, WHITE);
        let graph = (-10.0, -10.0, 40.0, 40.0);
        area_graph(&mut image, graph, &[1.0, 2.0], 4, 2.0, (WHITE, 0.0));
    }
}
