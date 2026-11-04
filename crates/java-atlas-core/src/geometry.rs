//! Plane geometry for the map: rectangles and the squarified treemap.
//!
//! A treemap divides a rectangle into smaller rectangles whose areas are
//! proportional to given values. The *squarified* variant (Bruls, Huizing,
//! van Wijk, 2000) keeps the pieces as close to squares as it can: long thin
//! slivers are hard to read, label and click — on a map, a region shaped like
//! a noodle is useless.

/// An axis-aligned rectangle in world units (y grows downwards, like SVG).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    pub fn area(&self) -> f64 {
        self.w * self.h
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// Shrinks the rectangle by `by` on every side, never below zero size.
    pub fn inset(&self, by: f64) -> Rect {
        let dx = by.min(self.w / 2.0);
        let dy = by.min(self.h / 2.0);
        Rect::new(
            self.x + dx,
            self.y + dy,
            self.w - 2.0 * dx,
            self.h - 2.0 * dy,
        )
    }

    /// Removes a band of `height` from the top (room for a label).
    pub fn below_header(&self, height: f64) -> Rect {
        let h = height.min(self.h);
        Rect::new(self.x, self.y + h, self.w, self.h - h)
    }

    /// Long side divided by short side: 1.0 is a square.
    pub fn aspect(&self) -> f64 {
        let (long, short) = if self.w >= self.h {
            (self.w, self.h)
        } else {
            (self.h, self.w)
        };
        if short <= 0.0 {
            f64::INFINITY
        } else {
            long / short
        }
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    pub fn contains(&self, other: &Rect) -> bool {
        const EPS: f64 = 1e-6;
        other.x >= self.x - EPS
            && other.y >= self.y - EPS
            && other.right() <= self.right() + EPS
            && other.bottom() <= self.bottom() + EPS
    }
}

/// The worst (largest) aspect ratio of a row of areas laid along a side of
/// length `side`. The squarify loop keeps adding to a row while this improves.
fn worst(row: &[f64], side: f64) -> f64 {
    let sum: f64 = row.iter().sum();
    let (min, max) = row.iter().fold((f64::INFINITY, 0.0f64), |(lo, hi), &a| {
        (lo.min(a), hi.max(a))
    });
    let s2 = sum * sum;
    let w2 = side * side;
    (w2 * max / s2).max(s2 / (w2 * min))
}

/// Places a finished row along the short side of `rect` (or as `column`
/// says); returns the space left and whether the row became a column.
fn place_row(
    row: &[(usize, f64)],
    rect: Rect,
    column: Option<bool>,
    out: &mut [Rect],
) -> (Rect, bool) {
    let sum: f64 = row.iter().map(|(_, a)| a).sum();
    if column.unwrap_or(rect.w >= rect.h) {
        // A column on the left edge.
        let width = if rect.h > 0.0 { sum / rect.h } else { 0.0 };
        let mut y = rect.y;
        for &(i, a) in row {
            let h = if width > 0.0 { a / width } else { 0.0 };
            out[i] = Rect::new(rect.x, y, width, h);
            y += h;
        }
        (
            Rect::new(rect.x + width, rect.y, rect.w - width, rect.h),
            true,
        )
    } else {
        // A row on the top edge.
        let height = if rect.w > 0.0 { sum / rect.w } else { 0.0 };
        let mut x = rect.x;
        for &(i, a) in row {
            let w = if height > 0.0 { a / height } else { 0.0 };
            out[i] = Rect::new(x, rect.y, w, height);
            x += w;
        }
        (
            Rect::new(rect.x, rect.y + height, rect.w, rect.h - height),
            false,
        )
    }
}

/// Where a tile ended up in its parent's treemap: in which row, and whether
/// that row was a column. Fed back into [`squarify_like`] (phase 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub row: u32,
    pub column: bool,
}

/// Splits `rect` into one rectangle per value, areas proportional to the
/// values, as square as possible. The result is in the order of `values`.
/// Zero or negative values get an empty rectangle at the corner.
pub fn squarify(values: &[f64], rect: Rect) -> Vec<Rect> {
    squarify_in_order(values, &biggest_first(values), rect)
}

/// Indices of `values`, biggest first; ties keep the input order, so equal
/// inputs always give the same picture. The order [`squarify`] lays out in.
pub fn biggest_first(values: &[f64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]).then(a.cmp(&b)));
    order
}

/// [`squarify`] with the placement order given by the caller (indices into
/// `values`). Biggest first gives the squarest tiles; keeping the order of
/// an earlier layout keeps tiles in place when sizes change a little
/// (phase 8: a live update must not shuffle the map).
pub fn squarify_in_order(values: &[f64], order: &[usize], rect: Rect) -> Vec<Rect> {
    squarify_like(values, order, &[], rect).0
}

/// [`squarify_in_order`] that also keeps the *rows* of an earlier layout:
/// tiles that shared a row (`previous`, indexed like `values`) share one
/// again, laid out in the same direction. Sizes only change the proportions;
/// the picture keeps its structure. Tiles without a previous placement are
/// grouped by the usual "squarest" rule. Returns the rectangles and the new
/// placements, both indexed like `values`.
pub fn squarify_like(
    values: &[f64],
    order: &[usize],
    previous: &[Option<Placement>],
    rect: Rect,
) -> (Vec<Rect>, Vec<Option<Placement>>) {
    let mut out = vec![Rect::new(rect.x, rect.y, 0.0, 0.0); values.len()];
    let mut placed = vec![None; values.len()];
    let total: f64 = values.iter().filter(|v| **v > 0.0).sum();
    if total <= 0.0 || rect.area() <= 0.0 {
        return (out, placed);
    }

    let scale = rect.area() / total;
    let order: Vec<(usize, f64)> = order
        .iter()
        .map(|&i| (i, values[i]))
        .filter(|(_, v)| *v > 0.0)
        .map(|(i, v)| (i, v * scale))
        .collect();
    let before = |i: usize| previous.get(i).copied().flatten();

    let mut free = rect;
    let mut row: Vec<(usize, f64)> = Vec::new();
    let mut row_id = 0;
    let mut flush = |row: &[(usize, f64)], free: Rect, row_id: &mut u32| {
        // The row keeps the direction its first tile had, if it had one.
        let column = row.first().and_then(|(i, _)| before(*i)).map(|p| p.column);
        let (rest, column) = place_row(row, free, column, &mut out);
        for (i, _) in row {
            placed[*i] = Some(Placement {
                row: *row_id,
                column,
            });
        }
        *row_id += 1;
        rest
    };
    for item in order {
        let join = match (row.first().and_then(|(i, _)| before(*i)), before(item.0)) {
            _ if row.is_empty() => true,
            (Some(first), Some(this)) => first.row == this.row,
            _ => {
                let side = free.w.min(free.h);
                let areas: Vec<f64> = row.iter().map(|(_, a)| *a).collect();
                let mut with: Vec<f64> = areas.clone();
                with.push(item.1);
                worst(&with, side) <= worst(&areas, side)
            }
        };
        if join {
            row.push(item);
        } else {
            free = flush(&row, free, &mut row_id);
            row = vec![item];
        }
    }
    if !row.is_empty() {
        flush(&row, free, &mut row_id);
    }
    (out, placed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn previous_rows_survive_a_change_in_size() {
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let values = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let order = biggest_first(&values);
        let (_, placed) = squarify_like(&values, &order, &[], rect);

        // The third tile almost doubles: plain squarify would regroup the rows.
        let grown = [6.0, 6.0, 7.5, 3.0, 2.0, 2.0, 1.0];
        let (rects, again) = squarify_like(&grown, &order, &placed, rect);
        assert_eq!(placed, again, "same rows, same directions");
        let (_, fresh) = squarify_like(&grown, &order, &[], rect);
        assert_ne!(placed, fresh, "without the hint the rows would change");
        let total: f64 = grown.iter().sum();
        for (v, r) in grown.iter().zip(&rects) {
            assert!(
                close(r.area(), v / total * rect.area()),
                "areas still follow the values"
            );
        }
    }

    #[test]
    fn areas_are_proportional_and_fill_the_rectangle() {
        let rect = Rect::new(10.0, 20.0, 600.0, 400.0);
        let values = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&values, rect);
        let total: f64 = values.iter().sum();
        for (v, r) in values.iter().zip(&rects) {
            assert!(close(r.area(), v / total * rect.area()), "{r:?}");
            assert!(rect.contains(r), "{r:?} escapes");
        }
        let covered: f64 = rects.iter().map(Rect::area).sum();
        assert!(close(covered, rect.area()));
    }

    #[test]
    fn pieces_do_not_overlap() {
        let rects = squarify(
            &[5.0, 4.0, 3.0, 3.0, 2.0, 1.0, 1.0, 1.0],
            Rect::new(0.0, 0.0, 300.0, 200.0),
        );
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                let overlap_w = a.right().min(b.right()) - a.x.max(b.x);
                let overlap_h = a.bottom().min(b.bottom()) - a.y.max(b.y);
                assert!(
                    overlap_w <= 1e-6 || overlap_h <= 1e-6,
                    "{a:?} overlaps {b:?}"
                );
            }
        }
    }

    #[test]
    fn the_textbook_example_stays_squarish() {
        // The example from the paper: 6 6 4 3 2 2 1 in a 6×4 rectangle.
        let rects = squarify(
            &[6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0],
            Rect::new(0.0, 0.0, 6.0, 4.0),
        );
        let worst = rects.iter().map(Rect::aspect).fold(0.0, f64::max);
        assert!(worst < 3.0, "worst aspect {worst}");
        // Slicing into strips would give 24:1 slivers for the smallest value.
    }

    #[test]
    fn one_value_takes_everything_and_zeros_take_nothing() {
        let rect = Rect::new(0.0, 0.0, 10.0, 5.0);
        assert_eq!(squarify(&[3.0], rect), vec![rect]);
        let rects = squarify(&[0.0, 2.0], rect);
        assert_eq!(rects[0].area(), 0.0);
        assert_eq!(rects[1], rect);
        assert!(squarify(&[], rect).is_empty());
    }

    #[test]
    fn result_order_follows_the_input() {
        let rects = squarify(&[1.0, 9.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert!(rects[1].area() > rects[0].area());
    }

    #[test]
    fn inset_never_inverts() {
        let r = Rect::new(0.0, 0.0, 4.0, 2.0).inset(5.0);
        assert!(r.w >= 0.0 && r.h >= 0.0);
    }
}
