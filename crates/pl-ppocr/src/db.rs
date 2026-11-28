//! DB (Differentiable Binarization) post-processing: probability map → text line quadrilaterals.
//!
//! Model: RapidOCR `ch_ppocr_det/utils.py` (`DBPostProcess`: thresh 0.3, box_thresh 0.5,
//! max_candidates 1000, unclip_ratio 1.6, 2×2 dilation, score_mode "fast") and
//! `ch_ppocr_det/main.py` (`sorted_boxes`). Contours are replaced by 8-connected components
//! (the outer contour of a component has the same convex hull, hence the same `minAreaRect`).

use crate::geom::{self, Quad};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DbParams {
    pub thresh: f32,
    pub box_thresh: f32,
    pub max_candidates: usize,
    pub unclip_ratio: f32,
    pub dilate: bool,
    pub min_size: f32,
}

impl Default for DbParams {
    fn default() -> Self {
        Self { thresh: 0.3, box_thresh: 0.5, max_candidates: 1000, unclip_ratio: 1.6, dilate: true, min_size: 3.0 }
    }
}

/// A detected text region in the coordinates of the detector input image.
#[derive(Debug, Clone, PartialEq)]
pub struct DetBox {
    pub quad: Quad,
    pub score: f32,
}

/// `pred`: probability map `h`×`w` (row-major). `dest_w`/`dest_h`: size of the image the boxes are
/// reported in (the image before resizing to the network size).
pub fn boxes_from_map(pred: &[f32], w: usize, h: usize, dest_w: usize, dest_h: usize, p: &DbParams) -> Vec<DetBox> {
    if w == 0 || h == 0 || pred.len() < w * h {
        return Vec::new();
    }
    let mut bitmap: Vec<u8> = pred[..w * h].iter().map(|&v| u8::from(v > p.thresh)).collect();
    if p.dilate {
        bitmap = dilate2x2(&bitmap, w, h);
    }
    let comps = components(&bitmap, w, h);
    let mut out = Vec::new();
    for pts in comps.iter().take(p.max_candidates) {
        let rect = geom::min_area_rect(pts);
        let (quad, sside) = geom::mini_box(&rect);
        if sside < p.min_size {
            continue;
        }
        let score = score_fast(pred, w, h, &quad);
        if p.box_thresh > score {
            continue;
        }
        let (area, peri) = geom::area_perimeter(&quad);
        let d = if peri > 0.0 { area * p.unclip_ratio / peri } else { 0.0 };
        let grown = rect.expanded(d);
        let (mut q, sside) = geom::mini_box(&grown);
        if sside < p.min_size + 2.0 {
            continue;
        }
        for pt in q.iter_mut() {
            pt[0] = (pt[0] / w as f32 * dest_w as f32).round().clamp(0.0, dest_w as f32);
            pt[1] = (pt[1] / h as f32 * dest_h as f32).round().clamp(0.0, dest_h as f32);
        }
        out.push(DetBox { quad: q, score });
    }
    filter(out, dest_w, dest_h)
}

/// `cv2.dilate` with a 2×2 kernel of ones (anchor at (1,1)): max over (x-1..=x, y-1..=y).
fn dilate2x2(b: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = b[y * w + x];
            if x > 0 {
                v |= b[y * w + x - 1];
            }
            if y > 0 {
                v |= b[(y - 1) * w + x];
                if x > 0 {
                    v |= b[(y - 1) * w + x - 1];
                }
            }
            out[y * w + x] = v;
        }
    }
    out
}

/// 8-connected components of the set pixels; per component the two end pixels of every run
/// (enough for the convex hull). Order: by first pixel in raster order.
fn components(b: &[u8], w: usize, h: usize) -> Vec<Vec<[f32; 2]>> {
    let mut label = vec![u32::MAX; w * h];
    let mut comps: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if b[start] == 0 || label[start] != u32::MAX {
            continue;
        }
        let id = comps.len() as u32;
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if b[j] != 0 && label[j] == u32::MAX {
                        label[j] = id;
                        stack.push(j);
                    }
                }
            }
        }
        comps.push(Vec::new());
    }
    for y in 0..h {
        let row = &label[y * w..(y + 1) * w];
        let mut x = 0;
        while x < w {
            let l = row[x];
            if l == u32::MAX {
                x += 1;
                continue;
            }
            let s = x;
            while x + 1 < w && row[x + 1] == l {
                x += 1;
            }
            if let Some(c) = comps.get_mut(l as usize) {
                c.push([s as f32, y as f32]);
                if x != s {
                    c.push([x as f32, y as f32]);
                }
            }
            x += 1;
        }
    }
    comps
}

/// RapidOCR `box_score_fast`: mean probability inside the (integer-truncated) quadrilateral.
fn score_fast(pred: &[f32], w: usize, h: usize, q: &Quad) -> f32 {
    let (bx0, by0, bx1, by1) = geom::bounds(q);
    let xmin = (bx0.floor() as isize).clamp(0, w as isize - 1);
    let xmax = (bx1.ceil() as isize).clamp(0, w as isize - 1);
    let ymin = (by0.floor() as isize).clamp(0, h as isize - 1);
    let ymax = (by1.ceil() as isize).clamp(0, h as isize - 1);
    let poly: Vec<[f32; 2]> = q.iter().map(|p| [p[0].trunc(), p[1].trunc()]).collect();
    let (mut sum, mut n) = (0f64, 0usize);
    for y in ymin..=ymax {
        let Some((lo, hi)) = geom::span_at(&poly, y as f32) else { continue };
        let x0 = (lo.ceil() as isize).max(xmin);
        let x1 = (hi.floor() as isize).min(xmax);
        for x in x0..=x1 {
            sum += pred[y as usize * w + x as usize] as f64;
            n += 1;
        }
    }
    if n == 0 { 0.0 } else { (sum / n as f64) as f32 }
}

/// RapidOCR `filter_det_res`: clockwise order, clip, drop boxes of at most 3 px.
fn filter(boxes: Vec<DetBox>, img_w: usize, img_h: usize) -> Vec<DetBox> {
    boxes
        .into_iter()
        .filter_map(|b| {
            let mut q = geom::order_clockwise(&b.quad);
            for p in q.iter_mut() {
                p[0] = p[0].clamp(0.0, img_w.saturating_sub(1) as f32).trunc();
                p[1] = p[1].clamp(0.0, img_h.saturating_sub(1) as f32).trunc();
            }
            let rw = geom::dist(q[0], q[1]) as i64;
            let rh = geom::dist(q[0], q[3]) as i64;
            (rw > 3 && rh > 3).then_some(DetBox { quad: q, score: b.score })
        })
        .collect()
}

/// RapidOCR `TextDetector.sorted_boxes`: top to bottom, boxes whose top-left y differs by less than
/// 10 px from the previous one form a line, left to right within it.
pub fn sort_boxes(mut boxes: Vec<DetBox>) -> Vec<DetBox> {
    boxes.sort_by(|a, b| a.quad[0][1].total_cmp(&b.quad[0][1]));
    let mut line = 0u32;
    let mut keyed: Vec<(u32, f32, DetBox)> = Vec::with_capacity(boxes.len());
    let mut prev_y: Option<f32> = None;
    for b in boxes {
        let y = b.quad[0][1];
        if let Some(py) = prev_y
            && y - py >= 10.0
        {
            line += 1;
        }
        prev_y = Some(y);
        keyed.push((line, b.quad[0][0], b));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    keyed.into_iter().map(|k| k.2).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_with_rects(w: usize, h: usize, rects: &[(usize, usize, usize, usize)]) -> Vec<f32> {
        let mut m = vec![0.0f32; w * h];
        for &(x0, y0, x1, y1) in rects {
            for y in y0..y1 {
                for x in x0..x1 {
                    m[y * w + x] = 0.9;
                }
            }
        }
        m
    }

    #[test]
    fn two_lines_found_scaled_and_sorted() {
        let (w, h) = (100, 60);
        let m = map_with_rects(w, h, &[(10, 30, 80, 40), (10, 10, 90, 20)]);
        let boxes = sort_boxes(boxes_from_map(&m, w, h, 200, 120, &DbParams::default()));
        assert_eq!(boxes.len(), 2);
        // first box is the upper line, scaled by 2, grown by unclip
        let (x0, y0, x1, y1) = geom::bounds(&boxes[0].quad);
        assert!(y0 < 20.0 && y1 > 38.0 && y1 < 60.0, "{:?}", boxes[0]);
        assert!(x0 < 20.0 && x1 > 178.0, "{:?}", boxes[0]);
        assert!(boxes[0].score > 0.75);
        assert!(geom::bounds(&boxes[1].quad).1 > 40.0);
    }

    #[test]
    fn tiny_and_weak_regions_dropped() {
        let (w, h) = (50, 50);
        let mut m = map_with_rects(w, h, &[(5, 5, 7, 7)]);
        // weak region: above thresh but below box_thresh
        for y in 20..30 {
            for x in 5..45 {
                m[y * w + x] = 0.35;
            }
        }
        assert!(boxes_from_map(&m, w, h, w, h, &DbParams::default()).is_empty());
    }

    #[test]
    fn dilation_merges_diagonal_gap() {
        let b = vec![1, 0, 0, 0, 0, 0, 0, 0, 1];
        let d = dilate2x2(&b, 3, 3);
        assert_eq!(d, vec![1, 1, 0, 1, 1, 0, 0, 0, 1]);
    }

    #[test]
    fn components_are_8_connected() {
        let b = vec![1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1];
        let c = components(&b, 4, 3);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn sort_groups_lines_by_10px() {
        let mk = |x: f32, y: f32| DetBox { quad: [[x, y], [x + 5.0, y], [x + 5.0, y + 5.0], [x, y + 5.0]], score: 1.0 };
        let s = sort_boxes(vec![mk(50.0, 3.0), mk(0.0, 0.0), mk(0.0, 30.0), mk(20.0, 8.0)]);
        let xs: Vec<f32> = s.iter().map(|b| b.quad[0][0]).collect();
        assert_eq!(xs, vec![0.0, 20.0, 50.0, 0.0]);
    }
}
