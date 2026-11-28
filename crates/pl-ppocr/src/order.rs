//! Reading order of text line boxes: recursive XY-cut.
//!
//! Port of the heuristic measured on 2026-11-24 (`results/ocr-eval/2026-11-24/tools/laptop/order.py`):
//! a vertical cut (column gutter) is taken only if both sides consist of at least three wide lines
//! (mean width >= 15 % of the page width), so table columns stay row-wise; otherwise cut at
//! horizontal gaps; leaves are grouped into rows (vertical overlap) and read left to right.
//! Without it the character error rate of two-column print rose from 5 to 47 %.

/// Axis-aligned box `(x0, y0, x1, y1)`.
pub type BBox = [f32; 4];

/// Result: blocks (XY-cut leaves) → rows → indices into the input, left to right.
pub type Order = Vec<Vec<Vec<usize>>>;

/// Reading order of `boxes` on a page of width `page_w`.
pub fn reading_order(boxes: &[BBox], page_w: f32) -> Order {
    let items: Vec<usize> = (0..boxes.len()).collect();
    let mut out = Vec::new();
    if !items.is_empty() {
        cut(boxes, &items, page_w, 0, &mut out);
    }
    out
}

fn cy(b: &BBox) -> f32 {
    (b[1] + b[3]) / 2.0
}
fn cx(b: &BBox) -> f32 {
    (b[0] + b[2]) / 2.0
}

/// Python `_rows`: group into rows by vertical centre (tolerance half the larger height of the row's
/// first box and the candidate), rows top to bottom, boxes left to right.
fn rows(boxes: &[BBox], items: &[usize]) -> Vec<Vec<usize>> {
    let mut sorted: Vec<usize> = items.to_vec();
    sorted.sort_by(|&a, &b| cy(&boxes[a]).total_cmp(&cy(&boxes[b])));
    // (row centre, row height, members) – centre/height of the first member, as in the Python code
    let mut rs: Vec<(f32, f32, Vec<usize>)> = Vec::new();
    for i in sorted {
        let b = &boxes[i];
        let (c, h) = (cy(b), b[3] - b[1]);
        if let Some(r) = rs.iter_mut().find(|r| (r.0 - c).abs() < 0.5 * h.max(r.1)) {
            r.2.push(i);
        } else {
            rs.push((c, h, vec![i]));
        }
    }
    rs.sort_by(|a, b| a.0.total_cmp(&b.0));
    rs.into_iter()
        .map(|mut r| {
            r.2.sort_by(|&a, &b| boxes[a][0].total_cmp(&boxes[b][0]));
            r.2
        })
        .collect()
}

/// Python `_gaps`: interior gaps of at least `min_gap` between the merged intervals.
fn gaps(mut iv: Vec<(f32, f32)>, lo: f32, min_gap: f32) -> Vec<(f32, f32)> {
    iv.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out = Vec::new();
    let mut cur = lo;
    for (a, b) in iv {
        if a - cur >= min_gap {
            out.push((cur, a));
        }
        cur = cur.max(b);
    }
    out.retain(|g| g.0 > lo);
    out
}

fn cut(boxes: &[BBox], items: &[usize], page_w: f32, depth: usize, out: &mut Order) {
    if items.len() <= 1 || depth > 30 {
        out.push(rows(boxes, items));
        return;
    }
    let f = |k: usize, max: bool| {
        items.iter().map(|&i| boxes[i][k]).fold(if max { f32::MIN } else { f32::MAX }, |a, v| if max { a.max(v) } else { a.min(v) })
    };
    let (xs0, ys0, ys1) = (f(0, false), f(1, false), f(3, true));
    let mut hs: Vec<f32> = items.iter().map(|&i| boxes[i][3] - boxes[i][1]).collect();
    hs.sort_by(f32::total_cmp);
    let hmed = hs[hs.len() / 2];
    // vertical cut (columns)
    let vg = gaps(items.iter().map(|&i| (boxes[i][0], boxes[i][2])).collect(), xs0, (0.015 * page_w).max(0.8 * hmed));
    if let Some(g) = vg.iter().copied().max_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0))) {
        let mid = (g.0 + g.1) / 2.0;
        let (l, r): (Vec<usize>, Vec<usize>) = items.iter().partition(|&&i| cx(&boxes[i]) < mid);
        let mw = |s: &[usize]| s.iter().map(|&i| boxes[i][2] - boxes[i][0]).sum::<f32>() / s.len().max(1) as f32;
        if l.len() >= 3 && r.len() >= 3 && mw(&l) >= 0.15 * page_w && mw(&r) >= 0.15 * page_w {
            cut(boxes, &l, page_w, depth + 1, out);
            cut(boxes, &r, page_w, depth + 1, out);
            return;
        }
    }
    // horizontal cuts
    let hg = gaps(items.iter().map(|&i| (boxes[i][1], boxes[i][3])).collect(), ys0, 0.6 * hmed);
    if !hg.is_empty() {
        let mut prev = ys0 - 1.0;
        let mut bounds: Vec<f32> = hg.iter().map(|g| g.0).collect();
        bounds.push(ys1 + 1.0);
        let mut parts: Vec<Vec<usize>> = Vec::new();
        for a in bounds {
            let part: Vec<usize> = items.iter().copied().filter(|&i| prev <= cy(&boxes[i]) && cy(&boxes[i]) < a).collect();
            prev = a;
            if !part.is_empty() {
                if part.len() == items.len() {
                    out.push(rows(boxes, items));
                    return;
                }
                parts.push(part);
            }
        }
        for p in parts {
            cut(boxes, &p, page_w, depth + 1, out);
        }
        return;
    }
    out.push(rows(boxes, items));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(o: &Order) -> Vec<usize> {
        o.iter().flatten().flatten().copied().collect()
    }

    #[test]
    fn two_columns_read_column_by_column() {
        // left column x 0..400, right column 500..900, 4 lines each, interleaved input order
        let mut b = Vec::new();
        for i in 0..4 {
            let y = 100.0 + i as f32 * 30.0;
            b.push([0.0, y, 400.0, y + 20.0]);
            b.push([500.0, y, 900.0, y + 20.0]);
        }
        let o = reading_order(&b, 1000.0);
        assert_eq!(flat(&o), vec![0, 2, 4, 6, 1, 3, 5, 7]);
    }

    #[test]
    fn table_columns_stay_row_wise() {
        // narrow cells (< 15 % of page width): rows must not be split into columns
        let mut b = Vec::new();
        for i in 0..4 {
            let y = 100.0 + i as f32 * 30.0;
            b.push([0.0, y, 80.0, y + 20.0]);
            b.push([500.0, y, 580.0, y + 20.0]);
        }
        let o = reading_order(&b, 1000.0);
        assert_eq!(flat(&o), vec![0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn rows_group_by_vertical_overlap() {
        let b = vec![[300.0, 12.0, 400.0, 30.0], [0.0, 10.0, 100.0, 30.0], [0.0, 50.0, 100.0, 70.0]];
        let o = reading_order(&b, 1000.0);
        let rows: Vec<Vec<usize>> = o.into_iter().flatten().collect();
        assert_eq!(rows, vec![vec![1, 0], vec![2]]);
    }

    #[test]
    fn empty_and_single() {
        assert!(reading_order(&[], 100.0).is_empty());
        assert_eq!(flat(&reading_order(&[[0.0, 0.0, 1.0, 1.0]], 100.0)), vec![0]);
    }
}
