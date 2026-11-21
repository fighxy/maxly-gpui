//! Album layout: where each photo or video of one message goes inside a grid, after the
//! grouped-media layout messengers use (Telegram's `LayoutMediaGroup` shape): two items side by
//! side or stacked, three as one large and two small, four as one large and three small, five
//! and more in rows picked to come closest to the height limit. Pure geometry, no UI.

/// One placed item, in px from the album's top left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// The whole album: its size and one cell per item, in item order.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Album width.
    pub width: f32,
    /// Album height.
    pub height: f32,
    /// One cell per item.
    pub cells: Vec<Cell>,
}

/// Size limits of an album.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Widest the album may be.
    pub max_width: f32,
    /// Tallest the album should be (rows may exceed it slightly for many items).
    pub max_height: f32,
    /// Narrowest a cell should be.
    pub min_width: f32,
    /// Gap between cells.
    pub spacing: f32,
}

/// The limits the history view uses.
pub const LIMITS: Limits = Limits {
    max_width: 400.,
    max_height: 400.,
    min_width: 90.,
    spacing: 2.,
};

/// Most items one grid holds; longer albums are stacked grids of this many.
const MAX_PER_GRID: usize = 10;

/// Width over height, with unknown sizes taken as square and extremes clamped.
fn ratio((w, h): (f32, f32)) -> f32 {
    if w > 0. && h > 0. {
        (w / h).clamp(0.25, 4.)
    } else {
        1.
    }
}

fn cell(x: f32, y: f32, w: f32, h: f32) -> Cell {
    Cell {
        x,
        y,
        w: w.max(1.),
        h: h.max(1.),
    }
}

/// Lay out items of the given pixel sizes (width, height; 0 when unknown).
pub fn layout(sizes: &[(f32, f32)], lim: &Limits) -> Layout {
    if sizes.len() > MAX_PER_GRID {
        // Stack grids of up to ten under each other.
        let mut out = Layout {
            width: 0.,
            height: 0.,
            cells: Vec::with_capacity(sizes.len()),
        };
        for chunk in sizes.chunks(MAX_PER_GRID) {
            let part = layout(chunk, lim);
            let top = if out.cells.is_empty() {
                0.
            } else {
                out.height + lim.spacing
            };
            out.cells
                .extend(part.cells.iter().map(|c| Cell { y: c.y + top, ..*c }));
            out.width = out.width.max(part.width);
            out.height = top + part.height;
        }
        return out;
    }
    let ratios: Vec<f32> = sizes.iter().copied().map(ratio).collect();
    let cells = match ratios.as_slice() {
        [] => Vec::new(),
        [r] => vec![one(*r, lim)],
        [a, b] => two(*a, *b, lim),
        [a, b, c] => three(*a, *b, *c, lim),
        [a, b, c, d] => four([*a, *b, *c, *d], lim),
        _ => many(&ratios, lim),
    };
    let width = cells.iter().map(|c| c.x + c.w).fold(0., f32::max);
    let height = cells.iter().map(|c| c.y + c.h).fold(0., f32::max);
    Layout {
        width,
        height,
        cells,
    }
}

/// 'w' wide, 'n' narrow, 'q' about square.
fn shape(r: f32) -> char {
    if r > 1.2 {
        'w'
    } else if r < 0.8 {
        'n'
    } else {
        'q'
    }
}

fn one(r: f32, lim: &Limits) -> Cell {
    let mut w = lim.max_width.min(lim.max_height * r);
    let mut h = w / r;
    if w < lim.min_width * 1.5 {
        // A very tall item keeps a usable width and is cropped in height.
        w = lim.min_width * 1.5;
        h = lim.max_height;
    }
    cell(0., 0., w.round(), h.min(lim.max_height).round())
}

fn two(r0: f32, r1: f32, lim: &Limits) -> Vec<Cell> {
    let s = lim.spacing;
    let avg = (r0 + r1) / 2.;
    let max_ratio = lim.max_width / lim.max_height;
    let shapes: String = [shape(r0), shape(r1)].iter().collect();
    if shapes == "ww" && avg > 1.4 * max_ratio && (r1 - r0).abs() < 0.2 {
        // Two wide ones: one above the other.
        let w = lim.max_width;
        let h = (w / r0).min(w / r1).min((lim.max_height - s) / 2.).round();
        vec![cell(0., 0., w, h), cell(0., h + s, w, h)]
    } else if shapes == "ww" || shapes == "qq" {
        // Alike: equal halves.
        let w = ((lim.max_width - s) / 2.).round();
        let h = (w / r0).min(w / r1).min(lim.max_height).round();
        vec![cell(0., 0., w, h), cell(w + s, 0., w, h)]
    } else {
        // Different: widths by ratio, both the same height.
        let free = lim.max_width - s;
        let second = (free / r0 / (1. / r0 + 1. / r1))
            .max(0.4 * free)
            .min(free - (lim.min_width * 1.5).round())
            .round();
        let first = free - second;
        let h = (first / r0).min(second / r1).min(lim.max_height).round();
        vec![cell(0., 0., first, h), cell(first + s, 0., second, h)]
    }
}

fn three(r0: f32, r1: f32, r2: f32, lim: &Limits) -> Vec<Cell> {
    let s = lim.spacing;
    if shape(r0) == 'n' {
        // A tall first item on the left, two on the right.
        let full = lim.max_height;
        let third = ((full - s) * 0.5)
            .min(r1 * (lim.max_width - s) / (r2 + r1))
            .round();
        let second = full - third - s;
        let right = ((lim.max_width - s) * 0.5)
            .min((third * r2).min(second * r1))
            .max(lim.min_width)
            .round();
        let left = (full * r0).min(lim.max_width - s - right).round();
        vec![
            cell(0., 0., left, full),
            cell(left + s, 0., right, second),
            cell(left + s, second + s, right, third),
        ]
    } else {
        // A wide first item on top, two below.
        let w = lim.max_width;
        let top = (w / r0).min((lim.max_height - s) * 0.66).round();
        let half = ((w - s) / 2.).round();
        let bottom = (lim.max_height - top - s)
            .min((half / r1).min(half / r2))
            .round();
        let rest = w - half - s;
        vec![
            cell(0., 0., w, top),
            cell(0., top + s, half, bottom),
            cell(half + s, top + s, rest, bottom),
        ]
    }
}

fn four(r: [f32; 4], lim: &Limits) -> Vec<Cell> {
    let s = lim.spacing;
    if shape(r[0]) == 'w' {
        // A wide first item on top, three in a row below.
        let w = lim.max_width;
        let top = (w / r[0]).min((lim.max_height - s) * 0.66).round();
        let free = w - 2. * s;
        let h = (free / (r[1] + r[2] + r[3])).round();
        let w0 = (free * 0.4).min(h * r[1]).max(lim.min_width).round();
        let w2 = (free * 0.33).max(lim.min_width).max(h * r[3]).round();
        let w1 = w - w0 - w2 - 2. * s;
        let h = h.min(lim.max_height - top - s);
        let y = top + s;
        vec![
            cell(0., 0., w, top),
            cell(0., y, w0, h),
            cell(w0 + s, y, w1, h),
            cell(w0 + w1 + 2. * s, y, w2, h),
        ]
    } else {
        // A tall first item on the left, three stacked on the right.
        let full = lim.max_height;
        let left = (full * r[0]).min((lim.max_width - s) * 0.6).round();
        let column = ((full - 2. * s) / (1. / r[1] + 1. / r[2] + 1. / r[3])).round();
        let h0 = (column / r[1]).round();
        let h1 = (column / r[2]).round();
        let h2 = full - h0 - h1 - 2. * s;
        let w = column
            .min(lim.max_width - left - s)
            .max(lim.min_width)
            .round();
        let x = left + s;
        vec![
            cell(0., 0., left, full),
            cell(x, 0., w, h0),
            cell(x, h0 + s, w, h1),
            cell(x, h0 + h1 + 2. * s, w, h2),
        ]
    }
}

/// Every way to split `n` items into 2–4 rows of 1–4 items each.
fn row_splits(n: usize) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    fn go(left: usize, rows: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if left == 0 {
            if rows.len() >= 2 {
                out.push(rows.clone());
            }
            return;
        }
        if rows.len() == 4 {
            return;
        }
        for k in 1..=left.min(4) {
            rows.push(k);
            go(left - k, rows, out);
            rows.pop();
        }
    }
    go(n, &mut Vec::new(), &mut out);
    out
}

fn many(ratios: &[f32], lim: &Limits) -> Vec<Cell> {
    let s = lim.spacing;
    let avg = ratios.iter().sum::<f32>() / ratios.len() as f32;
    // Mostly wide albums crop items to at least square, mostly tall ones to at most square.
    let cropped: Vec<f32> = ratios
        .iter()
        .map(|r| if avg > 1.1 { r.max(1.) } else { r.min(1.) })
        .collect();
    let row_height =
        |items: &[f32]| (lim.max_width - s * (items.len() as f32 - 1.)) / items.iter().sum::<f32>();
    let mut best: Option<(f32, Vec<usize>, Vec<f32>)> = None;
    for split in row_splits(cropped.len()) {
        let mut heights = Vec::with_capacity(split.len());
        let mut at = 0;
        for k in &split {
            heights.push(row_height(&cropped[at..at + k]));
            at += k;
        }
        let total = heights.iter().sum::<f32>() + s * (split.len() as f32 - 1.);
        let mut score = (total - lim.max_height).abs();
        // Rows should not get fuller towards the top, nor thinner than a usable cell.
        if split.windows(2).any(|w| w[0] > w[1]) {
            score *= 1.5;
        }
        if heights.iter().copied().fold(f32::MAX, f32::min) < lim.min_width {
            score *= 1.5;
        }
        if best.as_ref().is_none_or(|(b, _, _)| score < *b) {
            best = Some((score, split, heights));
        }
    }
    let Some((_, split, heights)) = best else {
        return Vec::new();
    };
    let mut cells = Vec::with_capacity(cropped.len());
    let (mut at, mut y) = (0, 0.);
    for (k, h) in split.iter().zip(heights) {
        let h = h.round();
        let mut x = 0.;
        for (i, r) in cropped[at..at + k].iter().enumerate() {
            // The last cell of a row takes what rounding left over, so rows end flush.
            let w = if i + 1 == *k {
                lim.max_width - x
            } else {
                (h * r).round()
            };
            cells.push(cell(x, y, w, h));
            x += w + s;
        }
        at += k;
        y += h + s;
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::{Cell, LIMITS, layout, row_splits};

    fn overlaps(a: &Cell, b: &Cell) -> bool {
        a.x < b.x + b.w - 0.5
            && b.x < a.x + a.w - 0.5
            && a.y < b.y + b.h - 0.5
            && b.y < a.y + a.h - 0.5
    }

    /// Every layout keeps its cells inside the album, apart, and the album within limits.
    fn check(sizes: &[(f32, f32)]) {
        let l = layout(sizes, &LIMITS);
        assert_eq!(l.cells.len(), sizes.len(), "{sizes:?}");
        assert!(
            l.width <= LIMITS.max_width + 0.5,
            "{sizes:?}: width {}",
            l.width
        );
        for (i, a) in l.cells.iter().enumerate() {
            assert!(a.w >= 1. && a.h >= 1., "{sizes:?}: {a:?}");
            assert!(a.x >= 0. && a.y >= 0., "{sizes:?}: {a:?}");
            assert!(a.x + a.w <= l.width + 0.5 && a.y + a.h <= l.height + 0.5);
            for b in &l.cells[i + 1..] {
                assert!(!overlaps(a, b), "{sizes:?}: {a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn all_counts_and_shapes_fit_without_overlap() {
        let shapes = [
            (1600., 900.),
            (900., 1600.),
            (1000., 1000.),
            (0., 0.),
            (4000., 300.),
        ];
        for n in 1..=12 {
            for start in 0..shapes.len() {
                let sizes: Vec<(f32, f32)> =
                    (0..n).map(|i| shapes[(start + i) % shapes.len()]).collect();
                check(&sizes);
            }
        }
    }

    #[test]
    fn single_items_keep_their_ratio() {
        let wide = layout(&[(1600., 900.)], &LIMITS);
        assert_eq!((wide.width, wide.height), (400., 225.));
        let tall = layout(&[(900., 1600.)], &LIMITS);
        assert_eq!(tall.height, 400.);
        assert!((tall.width - 225.).abs() < 1.);
    }

    #[test]
    fn two_squares_side_by_side_and_two_wide_ones_stacked() {
        let squares = layout(&[(1000., 1000.), (1000., 1000.)], &LIMITS);
        assert_eq!(squares.cells[0].y, squares.cells[1].y);
        assert_eq!(squares.cells[0].w, squares.cells[1].w);
        let wide = layout(&[(3000., 1000.), (3000., 1000.)], &LIMITS);
        assert_eq!(wide.cells[0].x, wide.cells[1].x, "stacked");
        assert!(wide.cells[1].y > wide.cells[0].y);
    }

    #[test]
    fn three_and_four_put_the_first_item_large() {
        let tall_first = layout(&[(900., 1600.), (1000., 1000.), (1000., 1000.)], &LIMITS);
        assert_eq!(tall_first.cells[0].h, LIMITS.max_height, "tall on the left");
        let wide_first = layout(
            &[
                (1600., 900.),
                (1000., 1000.),
                (1000., 1000.),
                (1000., 1000.),
            ],
            &LIMITS,
        );
        assert_eq!(wide_first.cells[0].w, LIMITS.max_width, "wide on top");
        let row: Vec<f32> = wide_first.cells[1..].iter().map(|c| c.y).collect();
        assert!(row.iter().all(|y| *y == row[0]), "three in a row below");
    }

    #[test]
    fn many_items_fill_rows_flush() {
        let l = layout(&[(1000., 1000.); 7], &LIMITS);
        for c in &l.cells {
            let row_end = l
                .cells
                .iter()
                .filter(|o| o.y == c.y)
                .map(|o| o.x + o.w)
                .fold(0., f32::max);
            assert_eq!(row_end, LIMITS.max_width);
        }
        let rows = l
            .cells
            .iter()
            .map(|c| c.y as i32)
            .collect::<std::collections::BTreeSet<_>>();
        assert!((2..=4).contains(&rows.len()));
        assert!(row_splits(5).iter().all(|s| s.iter().sum::<usize>() == 5));
    }
}
