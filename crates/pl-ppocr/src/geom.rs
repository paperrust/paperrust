//! Geometry helpers for the DB text detector: convex hull, minimum-area rectangle, point order.
//!
//! Model: OpenCV `cv2.minAreaRect`/`cv2.boxPoints` and RapidOCR `ch_ppocr_det/utils.py`
//! (`get_mini_boxes`, `order_points_clockwise`).

/// Quadrilateral, normally (top-left, top-right, bottom-right, bottom-left).
pub type Quad = [[f32; 2]; 4];

/// Rotated rectangle: centre, side lengths along `dir` (unit vector) and its normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotRect {
    pub cx: f32,
    pub cy: f32,
    /// Extent along `dir`.
    pub w: f32,
    /// Extent along the normal of `dir`.
    pub h: f32,
    pub dir: [f32; 2],
}

impl RotRect {
    pub fn corners(&self) -> Quad {
        let (ux, uy) = (self.dir[0] * self.w / 2.0, self.dir[1] * self.w / 2.0);
        let (vx, vy) = (-self.dir[1] * self.h / 2.0, self.dir[0] * self.h / 2.0);
        [
            [self.cx - ux - vx, self.cy - uy - vy],
            [self.cx + ux - vx, self.cy + uy - vy],
            [self.cx + ux + vx, self.cy + uy + vy],
            [self.cx - ux + vx, self.cy - uy + vy],
        ]
    }

    pub fn short_side(&self) -> f32 {
        self.w.min(self.h)
    }

    /// Grows both sides by `2 * d` (the minimum-area rectangle of a rectangle offset by `d` with
    /// round joins, as `pyclipper` + `minAreaRect` in RapidOCR's `unclip`).
    pub fn expanded(&self, d: f32) -> RotRect {
        RotRect { w: self.w + 2.0 * d, h: self.h + 2.0 * d, ..*self }
    }
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Convex hull (Andrew's monotone chain), counter-clockwise in a y-up frame, no collinear points.
pub fn convex_hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = points.to_vec();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let mut lower: Vec<[f32; 2]> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<[f32; 2]> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Minimum-area enclosing rectangle of a point set (rotating calipers over the hull edges).
pub fn min_area_rect(points: &[[f32; 2]]) -> RotRect {
    let hull = convex_hull(points);
    match hull.len() {
        0 => return RotRect { cx: 0.0, cy: 0.0, w: 0.0, h: 0.0, dir: [1.0, 0.0] },
        1 => return RotRect { cx: hull[0][0], cy: hull[0][1], w: 0.0, h: 0.0, dir: [1.0, 0.0] },
        _ => {}
    }
    let mut best: Option<(f32, RotRect)> = None;
    let n = hull.len();
    for i in 0..n {
        let a = hull[i];
        let b = hull[(i + 1) % n];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-9 {
            continue;
        }
        let u = [dx / len, dy / len];
        let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &hull {
            let pu = p[0] * u[0] + p[1] * u[1];
            let pv = -p[0] * u[1] + p[1] * u[0];
            umin = umin.min(pu);
            umax = umax.max(pu);
            vmin = vmin.min(pv);
            vmax = vmax.max(pv);
        }
        let area = (umax - umin) * (vmax - vmin);
        if best.as_ref().is_none_or(|(ba, _)| area < *ba) {
            let cu = (umin + umax) / 2.0;
            let cv = (vmin + vmax) / 2.0;
            let rect = RotRect { cx: cu * u[0] - cv * u[1], cy: cu * u[1] + cv * u[0], w: umax - umin, h: vmax - vmin, dir: u };
            best = Some((area, rect));
        }
    }
    best.map(|(_, r)| r).unwrap_or(RotRect { cx: hull[0][0], cy: hull[0][1], w: 0.0, h: 0.0, dir: [1.0, 0.0] })
}

/// RapidOCR `get_mini_boxes`: corners of the rectangle in the order (tl, tr, br, bl) after sorting
/// by x, and the short side.
pub fn mini_box(rect: &RotRect) -> (Quad, f32) {
    let mut pts = rect.corners();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let (i1, i4) = if pts[1][1] > pts[0][1] { (0, 1) } else { (1, 0) };
    let (i2, i3) = if pts[3][1] > pts[2][1] { (2, 3) } else { (3, 2) };
    ([pts[i1], pts[i2], pts[i3], pts[i4]], rect.short_side())
}

/// RapidOCR `order_points_clockwise` (tl, tr, br, bl).
pub fn order_clockwise(q: &Quad) -> Quad {
    let mut xs = *q;
    xs.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut left = [xs[0], xs[1]];
    let mut right = [xs[2], xs[3]];
    left.sort_by(|a, b| a[1].total_cmp(&b[1]));
    right.sort_by(|a, b| a[1].total_cmp(&b[1]));
    [left[0], right[0], right[1], left[1]]
}

pub fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Shoelace area and perimeter of a polygon.
pub fn area_perimeter(poly: &[[f32; 2]]) -> (f32, f32) {
    let n = poly.len();
    let mut a = 0.0;
    let mut p = 0.0;
    for i in 0..n {
        let (q, r) = (poly[i], poly[(i + 1) % n]);
        a += q[0] * r[1] - r[0] * q[1];
        p += dist(q, r);
    }
    (a.abs() / 2.0, p)
}

/// Axis-aligned bounds `(x0, y0, x1, y1)`.
pub fn bounds(q: &[[f32; 2]]) -> (f32, f32, f32, f32) {
    q.iter().fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |(a, b, c, d), p| (a.min(p[0]), b.min(p[1]), c.max(p[0]), d.max(p[1])))
}

/// Horizontal span `[x0, x1]` of a convex polygon at height `y` (`None` if `y` is outside).
pub fn span_at(poly: &[[f32; 2]], y: f32) -> Option<(f32, f32)> {
    let n = poly.len();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (ya, yb) = (a[1], b[1]);
        if (y < ya.min(yb)) || (y > ya.max(yb)) {
            continue;
        }
        if (yb - ya).abs() < 1e-9 {
            lo = lo.min(a[0].min(b[0]));
            hi = hi.max(a[0].max(b[0]));
        } else {
            let t = (y - ya) / (yb - ya);
            let x = a[0] + t * (b[0] - a[0]);
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    (lo <= hi).then_some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_of_square_with_inner_points() {
        let pts = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [1.0, 1.0], [1.0, 0.0]];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 4);
    }

    #[test]
    fn min_rect_axis_aligned() {
        let pts: Vec<[f32; 2]> = (0..=10).flat_map(|x| [[x as f32, 0.0], [x as f32, 4.0]]).collect();
        let r = min_area_rect(&pts);
        let (w, h) = if r.w > r.h { (r.w, r.h) } else { (r.h, r.w) };
        assert!((w - 10.0).abs() < 1e-4 && (h - 4.0).abs() < 1e-4, "{r:?}");
        assert!((r.cx - 5.0).abs() < 1e-4 && (r.cy - 2.0).abs() < 1e-4);
        let (q, s) = mini_box(&r);
        assert!((s - 4.0).abs() < 1e-4);
        // tl, tr, br, bl
        assert_eq!(q[0], [0.0, 0.0]);
        assert_eq!(q[2], [10.0, 4.0]);
    }

    #[test]
    fn min_rect_rotated() {
        // 45-degree rectangle 10 x 2
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let u = [c, c];
        let v = [-c, c];
        let pts: Vec<[f32; 2]> = [(-5.0, -1.0), (5.0, -1.0), (5.0, 1.0), (-5.0, 1.0)]
            .iter()
            .map(|&(a, b)| [100.0 + a * u[0] + b * v[0], 50.0 + a * u[1] + b * v[1]])
            .collect();
        let r = min_area_rect(&pts);
        assert!((r.w * r.h - 20.0).abs() < 1e-2, "{r:?}");
        assert!((r.short_side() - 2.0).abs() < 1e-3);
        let e = r.expanded(1.0);
        assert!((e.w * e.h - 48.0).abs() < 1e-2);
    }

    #[test]
    fn order_and_measure() {
        let q = [[10.0, 5.0], [0.0, 0.0], [0.0, 5.0], [10.0, 0.0]];
        assert_eq!(order_clockwise(&q), [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
        let (a, p) = area_perimeter(&order_clockwise(&q));
        assert_eq!((a, p), (50.0, 30.0));
        assert_eq!(bounds(&q), (0.0, 0.0, 10.0, 5.0));
        assert_eq!(span_at(&order_clockwise(&q), 2.0), Some((0.0, 10.0)));
        assert_eq!(span_at(&order_clockwise(&q), 6.0), None);
    }
}
