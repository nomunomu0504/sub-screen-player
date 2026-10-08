//! Finding the parts of a picture that changed, so a display that takes partial images can be
//! sent only those (see [`crate::Capabilities::partial_images`]).

use image::RgbImage;

/// Side of the square tiles pictures are compared in. JPEG works in blocks of up to 16x16
/// pixels (4:2:0), so regions on this grid are encoded exactly like the same area of a whole
/// frame and no seams show at their edges.
pub const TILE: u32 = 16;

/// A rectangle on the panel, in the panel's own orientation (the encoded image's pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Rect {
    /// Number of pixels.
    pub fn area(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }
}

/// How the next picture differs from the one before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Nothing changed.
    None,
    /// Only these regions changed; together they cover at most half of the picture.
    Regions(Vec<Rect>),
    /// Too much changed to be worth sending in parts (or the sizes differ).
    Whole,
}

/// Compares `before` and `after` in [`TILE`]-sized tiles and merges the changed ones into at
/// most `max_regions` rectangles aligned to the tiles. Changes covering more than half of the
/// picture come back as [`Change::Whole`].
pub fn changes(before: &RgbImage, after: &RgbImage, max_regions: usize) -> Change {
    if before.dimensions() != after.dimensions() || max_regions == 0 {
        return Change::Whole;
    }
    let (width, height) = after.dimensions();
    let columns = width.div_ceil(TILE) as usize;
    let rows = height.div_ceil(TILE);
    let row_bytes = width as usize * 3;
    let (old, new) = (before.as_raw(), after.as_raw());

    // One box per run of tile rows that have changes, as wide as the changes in the run.
    let mut boxes: Vec<[u32; 4]> = Vec::new(); // first column, first row, end column, end row
    for row in 0..rows {
        let mut first = usize::MAX;
        let mut last = 0;
        for y in row * TILE..((row + 1) * TILE).min(height) {
            let start = y as usize * row_bytes;
            let (a, b) = (
                &old[start..start + row_bytes],
                &new[start..start + row_bytes],
            );
            if a == b {
                continue;
            }
            for column in 0..columns {
                let from = column * TILE as usize * 3;
                let to = (from + TILE as usize * 3).min(row_bytes);
                if a[from..to] != b[from..to] {
                    first = first.min(column);
                    last = last.max(column);
                }
            }
        }
        if first == usize::MAX {
            continue;
        }
        let (first, end) = (first as u32, last as u32 + 1);
        match boxes.last_mut() {
            Some(b) if b[3] == row => {
                b[0] = b[0].min(first);
                b[2] = b[2].max(end);
                b[3] = row + 1;
            }
            _ => boxes.push([first, row, end, row + 1]),
        }
    }
    if boxes.is_empty() {
        return Change::None;
    }

    // Merge neighbours that add the least area until few enough boxes are left.
    let area = |b: &[u32; 4]| u64::from(b[2] - b[0]) * u64::from(b[3] - b[1]);
    while boxes.len() > max_regions {
        let (i, merged) = (0..boxes.len() - 1)
            .map(|i| {
                let (p, q) = (boxes[i], boxes[i + 1]);
                let m = [p[0].min(q[0]), p[1], p[2].max(q[2]), q[3]];
                (area(&m) - area(&p) - area(&q), i, m)
            })
            .min_by_key(|&(extra, ..)| extra)
            .map(|(_, i, m)| (i, m))
            .expect("at least two boxes");
        boxes.splice(i..i + 2, [merged]);
    }

    let regions: Vec<Rect> = boxes
        .iter()
        .map(|b| {
            let (x, y) = (b[0] * TILE, b[1] * TILE);
            Rect {
                x,
                y,
                width: (b[2] * TILE).min(width) - x,
                height: (b[3] * TILE).min(height) - y,
            }
        })
        .collect();
    let changed: u64 = regions.iter().map(Rect::area).sum();
    if changed * 2 > u64::from(width) * u64::from(height) {
        return Change::Whole;
    }
    Change::Regions(regions)
}

#[cfg(test)]
mod tests {
    use image::Rgb;

    use super::*;

    fn picture() -> RgbImage {
        RgbImage::from_fn(462, 1920, |x, y| Rgb([(x % 251) as u8, (y % 241) as u8, 7]))
    }

    fn paint(image: &mut RgbImage, x: u32, y: u32, w: u32, h: u32) {
        for py in y..y + h {
            for px in x..x + w {
                image.put_pixel(px, py, Rgb([255, 0, 255]));
            }
        }
    }

    #[test]
    fn finds_no_change() {
        assert_eq!(changes(&picture(), &picture(), 4), Change::None);
    }

    #[test]
    fn aligns_a_change_to_tiles() {
        let mut after = picture();
        paint(&mut after, 20, 35, 3, 2);
        let rect = Rect {
            x: 16,
            y: 32,
            width: 16,
            height: 16,
        };
        assert_eq!(changes(&picture(), &after, 4), Change::Regions(vec![rect]));
    }

    #[test]
    fn keeps_edge_tiles_inside_the_picture() {
        // 462 is not a multiple of 16: the last column of tiles is 14 pixels wide.
        let mut after = picture();
        paint(&mut after, 461, 1919, 1, 1);
        let rect = Rect {
            x: 448,
            y: 1904,
            width: 14,
            height: 16,
        };
        assert_eq!(changes(&picture(), &after, 4), Change::Regions(vec![rect]));
    }

    #[test]
    fn separate_changes_become_separate_regions_up_to_the_limit() {
        let mut after = picture();
        paint(&mut after, 0, 0, 10, 10);
        paint(&mut after, 300, 900, 10, 10);
        paint(&mut after, 100, 1800, 10, 10);
        let Change::Regions(three) = changes(&picture(), &after, 4) else {
            panic!("expected regions");
        };
        assert_eq!(three.len(), 3);
        let Change::Regions(two) = changes(&picture(), &after, 2) else {
            panic!("expected regions");
        };
        assert_eq!(two.len(), 2);
        // Every changed pixel is inside a region.
        for (x, y) in [(5, 5), (305, 905), (105, 1805)] {
            assert!(
                two.iter()
                    .any(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            );
        }
    }

    #[test]
    fn large_changes_send_the_whole_picture() {
        let mut after = picture();
        paint(&mut after, 0, 0, 462, 1000);
        assert_eq!(changes(&picture(), &after, 4), Change::Whole);
        assert_eq!(
            changes(&picture(), &RgbImage::new(10, 10), 4),
            Change::Whole
        );
    }
}
