//! Minimal image operations needed by the PP-OCR pipeline (no OpenCV).
//!
//! Model: RapidOCR `utils/process_img.py`, `ch_ppocr_det/utils.py` (`DetPreProcess`),
//! `ch_ppocr_rec/main.py` (`resize_norm_img`). OpenCV's `INTER_LINEAR` resize (half-pixel centres,
//! no anti-aliasing) is reproduced; the perspective crop samples bilinearly where RapidOCR uses
//! `INTER_CUBIC` (see "Known differences" in `docs/OCR-PPOCR.md`).

/// 8-bit image, interleaved, 1 (gray) or 3 (RGB) channels, origin top left.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub data: Vec<u8>,
}

impl Image {
    /// Wraps a buffer; `None` if the size does not match or channels are not 1 or 3.
    pub fn new(width: usize, height: usize, channels: usize, data: Vec<u8>) -> Option<Self> {
        if !(channels == 1 || channels == 3) || width.checked_mul(height)?.checked_mul(channels)? != data.len() {
            return None;
        }
        Some(Self { width, height, channels, data })
    }

    pub fn filled(width: usize, height: usize, channels: usize, value: u8) -> Self {
        Self { width, height, channels, data: vec![value; width * height * channels] }
    }

    /// Pixel as RGB (gray is replicated); coordinates are clamped (`BORDER_REPLICATE`).
    #[inline]
    pub fn rgb_clamped(&self, x: isize, y: isize) -> [u8; 3] {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        let i = (y * self.width + x) * self.channels;
        if self.channels == 1 {
            let v = self.data[i];
            [v, v, v]
        } else {
            [self.data[i], self.data[i + 1], self.data[i + 2]]
        }
    }

    /// Resize like `cv2.resize(..., INTER_LINEAR)`.
    pub fn resize(&self, nw: usize, nh: usize) -> Image {
        let nw = nw.max(1);
        let nh = nh.max(1);
        if nw == self.width && nh == self.height {
            return self.clone();
        }
        let c = self.channels;
        let sx = self.width as f32 / nw as f32;
        let sy = self.height as f32 / nh as f32;
        // Precompute horizontal taps.
        let xt: Vec<(usize, usize, f32)> = (0..nw)
            .map(|x| {
                let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
                let x0 = (fx.floor() as usize).min(self.width - 1);
                let x1 = (x0 + 1).min(self.width - 1);
                (x0, x1, fx - x0 as f32)
            })
            .collect();
        let mut out = vec![0u8; nw * nh * c];
        for y in 0..nh {
            let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
            let y0 = (fy.floor() as usize).min(self.height - 1);
            let y1 = (y0 + 1).min(self.height - 1);
            let wy = fy - y0 as f32;
            let r0 = &self.data[y0 * self.width * c..(y0 + 1) * self.width * c];
            let r1 = &self.data[y1 * self.width * c..(y1 + 1) * self.width * c];
            let orow = &mut out[y * nw * c..(y + 1) * nw * c];
            for (x, &(x0, x1, wx)) in xt.iter().enumerate() {
                for k in 0..c {
                    let a = r0[x0 * c + k] as f32 * (1.0 - wx) + r0[x1 * c + k] as f32 * wx;
                    let b = r1[x0 * c + k] as f32 * (1.0 - wx) + r1[x1 * c + k] as f32 * wx;
                    orow[x * c + k] = (a * (1.0 - wy) + b * wy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Image { width: nw, height: nh, channels: c, data: out }
    }

    /// `np.rot90` (counter-clockwise by 90 degrees).
    pub fn rot90_ccw(&self) -> Image {
        let (w, h, c) = (self.width, self.height, self.channels);
        let mut out = vec![0u8; w * h * c];
        // new[y'][x'] = old[x'][w-1-y']; new size (h, w)
        for ny in 0..w {
            for nx in 0..h {
                let ox = w - 1 - ny;
                let oy = nx;
                let si = (oy * w + ox) * c;
                let di = (ny * h + nx) * c;
                out[di..di + c].copy_from_slice(&self.data[si..si + c]);
            }
        }
        Image { width: h, height: w, channels: c, data: out }
    }

    /// Clockwise rotation by 0/90/180/270 degrees.
    pub fn rotate_cw(&self, degrees: u16) -> Image {
        match degrees % 360 {
            90 => self.rot90_ccw().rot90_ccw().rot90_ccw(),
            180 => self.rot90_ccw().rot90_ccw(),
            270 => self.rot90_ccw(),
            _ => self.clone(),
        }
    }

    /// Adds `top`/`bottom` black rows (`cv2.copyMakeBorder(..., BORDER_CONSTANT, 0)`).
    pub fn pad_vertical(&self, top: usize, bottom: usize) -> Image {
        let row = self.width * self.channels;
        let mut data = vec![0u8; row * (self.height + top + bottom)];
        data[row * top..row * (top + self.height)].copy_from_slice(&self.data);
        Image { width: self.width, height: self.height + top + bottom, channels: self.channels, data }
    }

    /// Crops the quadrilateral `quad` (tl, tr, br, bl) to a `w`×`h` rectangle through a perspective
    /// transform (`cv2.warpPerspective` with `BORDER_REPLICATE`, bilinear). Returns the image and the
    /// homography that maps crop coordinates back to source coordinates.
    pub fn warp_quad(&self, quad: &[[f32; 2]; 4], w: usize, h: usize) -> Option<(Image, Homography)> {
        let w = w.max(1);
        let h = h.max(1);
        let dst = [[0.0, 0.0], [w as f32, 0.0], [w as f32, h as f32], [0.0, h as f32]];
        let m = Homography::from_points(&dst, quad)?;
        let c = self.channels;
        let mut out = vec![0u8; w * h * c];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = m.apply(x as f32, y as f32);
                let x0 = sx.floor();
                let y0 = sy.floor();
                let fx = sx - x0;
                let fy = sy - y0;
                let (x0, y0) = (x0 as isize, y0 as isize);
                let p00 = self.rgb_clamped(x0, y0);
                let p10 = self.rgb_clamped(x0 + 1, y0);
                let p01 = self.rgb_clamped(x0, y0 + 1);
                let p11 = self.rgb_clamped(x0 + 1, y0 + 1);
                let di = (y * w + x) * c;
                for k in 0..c {
                    let a = p00[k] as f32 * (1.0 - fx) + p10[k] as f32 * fx;
                    let b = p01[k] as f32 * (1.0 - fx) + p11[k] as f32 * fx;
                    out[di + k] = (a * (1.0 - fy) + b * fy).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Some((Image { width: w, height: h, channels: c, data: out }, m))
    }

    /// CHW float tensor data: `(px * scale - mean[k]) / std[k]`, channel order BGR if `bgr`
    /// (PaddleOCR det/rec are trained on OpenCV BGR images), otherwise RGB.
    pub fn to_chw(&self, scale: f32, mean: [f32; 3], std: [f32; 3], bgr: bool) -> Vec<f32> {
        let n = self.width * self.height;
        let mut out = vec![0f32; 3 * n];
        self.write_chw(&mut out, self.width, scale, mean, std, bgr);
        out
    }

    /// Writes the CHW data into `dst` laid out with row stride `stride` (>= width) per channel plane
    /// of `stride * height` values; the rest of each row is left as is (padding).
    pub fn write_chw(&self, dst: &mut [f32], stride: usize, scale: f32, mean: [f32; 3], std: [f32; 3], bgr: bool) {
        let plane = stride * self.height;
        let order: [usize; 3] = if bgr { [2, 1, 0] } else { [0, 1, 2] };
        for y in 0..self.height {
            for x in 0..self.width {
                let i = (y * self.width + x) * self.channels;
                let px = if self.channels == 1 {
                    let v = self.data[i];
                    [v, v, v]
                } else {
                    [self.data[i], self.data[i + 1], self.data[i + 2]]
                };
                for (k, &src) in order.iter().enumerate() {
                    dst[k * plane + y * stride + x] = (px[src] as f32 * scale - mean[k]) / std[k];
                }
            }
        }
    }
}

/// 3×3 projective transform (row-major, `h[8]` normalised to 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    /// Transform that maps `src[i]` to `dst[i]` (`cv2.getPerspectiveTransform`).
    pub fn from_points(src: &[[f32; 2]; 4], dst: &[[f32; 2]; 4]) -> Option<Self> {
        let mut a = [[0f64; 9]; 8];
        for i in 0..4 {
            let (x, y) = (src[i][0] as f64, src[i][1] as f64);
            let (u, v) = (dst[i][0] as f64, dst[i][1] as f64);
            a[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -x * u, -y * u, u];
            a[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -x * v, -y * v, v];
        }
        // Gaussian elimination with partial pivoting.
        for col in 0..8 {
            let piv = (col..8).max_by(|&p, &q| a[p][col].abs().total_cmp(&a[q][col].abs()))?;
            if a[piv][col].abs() < 1e-12 {
                return None;
            }
            a.swap(col, piv);
            for r in 0..8 {
                if r != col {
                    let f = a[r][col] / a[col][col];
                    if f != 0.0 {
                        let pivot_row = a[col];
                        for (v, p) in a[r][col..].iter_mut().zip(&pivot_row[col..]) {
                            *v -= f * p;
                        }
                    }
                }
            }
        }
        let mut h = [0f64; 9];
        for i in 0..8 {
            h[i] = a[i][8] / a[i][i];
        }
        h[8] = 1.0;
        Some(Self(h))
    }

    #[inline]
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let h = &self.0;
        let (x, y) = (x as f64, y as f64);
        let z = h[6] * x + h[7] * y + h[8];
        let z = if z.abs() < 1e-12 { 1e-12 } else { z };
        (((h[0] * x + h[1] * y + h[2]) / z) as f32, ((h[3] * x + h[4] * y + h[5]) / z) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(w: usize, h: usize) -> Image {
        let data = (0..w * h).map(|i| ((i % w) * 255 / (w - 1).max(1)) as u8).collect();
        Image::new(w, h, 1, data).unwrap()
    }

    #[test]
    fn new_checks_size() {
        assert!(Image::new(2, 2, 3, vec![0; 12]).is_some());
        assert!(Image::new(2, 2, 3, vec![0; 11]).is_none());
        assert!(Image::new(2, 2, 2, vec![0; 8]).is_none());
    }

    #[test]
    fn resize_identity_and_halving() {
        let img = ramp(8, 2);
        assert_eq!(img.resize(8, 2), img);
        let half = img.resize(4, 1);
        assert_eq!((half.width, half.height), (4, 1));
        // half-pixel centres: dst x=0 samples src x=0.5 → between 0 and 36
        assert_eq!(half.data[0], ((0.0f32 + 36.0) / 2.0).round() as u8);
        // monotone ramp stays monotone
        assert!(half.data.windows(2).all(|p| p[0] <= p[1]));
    }

    #[test]
    fn rotations_round_trip() {
        let img = Image::new(3, 2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap();
        let r = img.rot90_ccw();
        assert_eq!((r.width, r.height), (2, 3));
        // np.rot90([[1,2,3],[4,5,6]]) == [[3,6],[2,5],[1,4]]
        assert_eq!(r.data, vec![3, 6, 2, 5, 1, 4]);
        assert_eq!(img.rotate_cw(90).data, vec![4, 1, 5, 2, 6, 3]);
        assert_eq!(img.rotate_cw(90).rotate_cw(270), img);
        assert_eq!(img.rotate_cw(180).rotate_cw(180), img);
    }

    #[test]
    fn homography_maps_corners() {
        let src = [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]];
        let dst = [[2.0, 3.0], [22.0, 5.0], [21.0, 15.0], [1.0, 13.0]];
        let h = Homography::from_points(&src, &dst).unwrap();
        for i in 0..4 {
            let (u, v) = h.apply(src[i][0], src[i][1]);
            assert!((u - dst[i][0]).abs() < 1e-3 && (v - dst[i][1]).abs() < 1e-3, "{i}: {u},{v}");
        }
        assert!(Homography::from_points(&src, &[[0.0, 0.0]; 4]).is_none());
    }

    #[test]
    fn warp_axis_aligned_crop_is_copy() {
        let img = Image::new(4, 3, 1, (0..12).collect()).unwrap();
        let q = [[1.0, 1.0], [3.0, 1.0], [3.0, 2.0], [1.0, 2.0]];
        let (c, _) = img.warp_quad(&q, 2, 1).unwrap();
        assert_eq!(c.data, vec![5, 6]);
    }

    #[test]
    fn chw_normalisation_and_bgr() {
        let img = Image::new(1, 1, 3, vec![255, 0, 51]).unwrap();
        let v = img.to_chw(1.0 / 255.0, [0.5; 3], [0.5; 3], true);
        assert_eq!(v.len(), 3);
        assert!((v[0] - (0.2 - 0.5) / 0.5).abs() < 1e-6); // B first
        assert!((v[2] - 1.0).abs() < 1e-6); // R last
    }

    #[test]
    fn pad_vertical_adds_black_rows() {
        let img = Image::filled(2, 1, 1, 9);
        let p = img.pad_vertical(1, 2);
        assert_eq!(p.height, 4);
        assert_eq!(p.data, vec![0, 0, 9, 9, 0, 0, 0, 0]);
    }
}
