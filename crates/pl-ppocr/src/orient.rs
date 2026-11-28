//! Page orientation (0/90/180/270°) with a small classifier, run before any text recognition.
//!
//! Two models are supported (stage 1 of `docs/OCR-NEXT.md` compares them):
//! - PP-LCNet_x1_0_doc_ori (PaddleX; ONNX export from the oar-ocr project). Model: PaddleX
//!   `inference.yml` of the model (`ResizeImage resize_short 256`, `CropImage 224`, ImageNet
//!   mean/std, RGB) and `doc_preprocessor` (`rotate_image(img, angle)`, angle counter-clockwise).
//! - docTR/OnnxTR `mobilenet_v3_small_page_orientation` (`onnxtr/models/classification`: 512×512,
//!   aspect ratio kept, symmetric black padding, mean 0.694/0.695/0.693, std 0.299/0.296/0.301,
//!   classes `[0, -90, 180, 90]`).

use crate::image::Image;
use crate::runtime::{Model, Tensor};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientModel {
    PpLcnet,
    DoctrMobilenet,
}

/// Classifier result: clockwise rotation that straightens the page and the class probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageOrientation {
    pub rotate_cw: u16,
    pub prob: f32,
}

pub struct Orientation {
    model: Box<dyn Model>,
    kind: OrientModel,
}

impl Orientation {
    pub fn new(model: Box<dyn Model>, kind: OrientModel) -> Self {
        Self { model, kind }
    }

    pub fn kind(&self) -> OrientModel {
        self.kind
    }

    pub fn classify(&self, img: &Image) -> Result<PageOrientation> {
        if img.width == 0 || img.height == 0 {
            return Ok(PageOrientation { rotate_cw: 0, prob: 0.0 });
        }
        let input = match self.kind {
            OrientModel::PpLcnet => pplcnet_input(img)?,
            OrientModel::DoctrMobilenet => doctr_input(img)?,
        };
        let out = self.model.run(input)?;
        if out.data.len() < 4 {
            return Err(Error::Model(format!("orientation output has {} values, expected 4", out.data.len())));
        }
        let probs = softmax_if_needed(&out.data[..4]);
        let (best, p) = probs.iter().copied().enumerate().fold((0, f32::MIN), |a, (i, v)| if v > a.1 { (i, v) } else { a });
        Ok(PageOrientation { rotate_cw: rotation_for_class(self.kind, best), prob: p })
    }
}

/// Clockwise correction for a class index.
pub fn rotation_for_class(kind: OrientModel, class: usize) -> u16 {
    match kind {
        // labels 0/90/180/270 = the page must be turned that far counter-clockwise
        OrientModel::PpLcnet => [0, 270, 180, 90][class.min(3)],
        // classes [0, -90, 180, 90]; measured on the corpus (stage 1): class -90 needs a 90°
        // clockwise turn, class 90 a 270° one
        OrientModel::DoctrMobilenet => [0, 90, 180, 270][class.min(3)],
    }
}

fn softmax_if_needed(v: &[f32]) -> Vec<f32> {
    let sum: f32 = v.iter().sum();
    if v.iter().all(|x| (0.0..=1.0).contains(x)) && (sum - 1.0).abs() < 1e-3 {
        return v.to_vec();
    }
    let m = v.iter().copied().fold(f32::MIN, f32::max);
    let e: Vec<f32> = v.iter().map(|x| (x - m).exp()).collect();
    let s: f32 = e.iter().sum();
    e.into_iter().map(|x| x / s).collect()
}

fn pplcnet_input(img: &Image) -> Result<Tensor> {
    let (w, h) = (img.width as f32, img.height as f32);
    let scale = 256.0 / w.min(h);
    let (nw, nh) = (((w * scale).round() as usize).max(224), ((h * scale).round() as usize).max(224));
    let r = img.resize(nw, nh);
    let (x0, y0) = ((nw - 224) / 2, (nh - 224) / 2);
    let mut crop = Vec::with_capacity(224 * 224 * r.channels);
    for y in y0..y0 + 224 {
        let s = (y * nw + x0) * r.channels;
        crop.extend_from_slice(&r.data[s..s + 224 * r.channels]);
    }
    let c = Image::new(224, 224, r.channels, crop).ok_or_else(|| Error::Model("crop size".into()))?;
    let data = c.to_chw(1.0 / 255.0, [0.485, 0.456, 0.406], [0.229, 0.224, 0.225], false);
    Tensor::new(vec![1, 3, 224, 224], data)
}

fn doctr_input(img: &Image) -> Result<Tensor> {
    const S: usize = 512;
    let (w, h) = (img.width as f32, img.height as f32);
    let scale = (S as f32 / w).min(S as f32 / h);
    let (nw, nh) = (((w * scale).round() as usize).clamp(1, S), ((h * scale).round() as usize).clamp(1, S));
    let r = area_resize(img, nw, nh);
    let (ox, oy) = ((S - nw) / 2, (S - nh) / 2);
    let mean = [0.694, 0.695, 0.693];
    let std = [0.299, 0.296, 0.301];
    // padded pixels are 0 before normalisation
    let mut data = vec![0f32; 3 * S * S];
    for k in 0..3 {
        let v = (0.0 - mean[k]) / std[k];
        data[k * S * S..(k + 1) * S * S].iter_mut().for_each(|x| *x = v);
    }
    for y in 0..nh {
        for x in 0..nw {
            let i = (y * nw + x) * r.channels;
            let px = if r.channels == 1 { [r.data[i]; 3] } else { [r.data[i], r.data[i + 1], r.data[i + 2]] };
            for k in 0..3 {
                data[k * S * S + (y + oy) * S + x + ox] = (px[k] as f32 / 255.0 - mean[k]) / std[k];
            }
        }
    }
    Tensor::new(vec![1, 3, S, S], data)
}

/// Box-filter downscale (like PIL's anti-aliased resize for large factors); upscales fall back to
/// bilinear.
pub fn area_resize(img: &Image, nw: usize, nh: usize) -> Image {
    if nw >= img.width || nh >= img.height {
        return img.resize(nw, nh);
    }
    let c = img.channels;
    let sx = img.width as f32 / nw as f32;
    let sy = img.height as f32 / nh as f32;
    let mut out = vec![0u8; nw * nh * c];
    for y in 0..nh {
        let y0 = (y as f32 * sy) as usize;
        let y1 = (((y + 1) as f32 * sy) as usize).clamp(y0 + 1, img.height);
        for x in 0..nw {
            let x0 = (x as f32 * sx) as usize;
            let x1 = (((x + 1) as f32 * sx) as usize).clamp(x0 + 1, img.width);
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            for k in 0..c {
                let mut s = 0u32;
                for yy in y0..y1 {
                    let row = &img.data[(yy * img.width) * c..];
                    for xx in x0..x1 {
                        s += row[xx * c + k] as u32;
                    }
                }
                out[(y * nw + x) * c + k] = ((s + n / 2) / n) as u8;
            }
        }
    }
    Image { width: nw, height: nh, channels: c, data: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_only_for_logits() {
        let p = softmax_if_needed(&[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(p, vec![0.1, 0.2, 0.3, 0.4]);
        let l = softmax_if_needed(&[2.0, 0.0, 0.0, 0.0]);
        assert!((l.iter().sum::<f32>() - 1.0).abs() < 1e-5 && l[0] > 0.7);
    }

    #[test]
    fn class_to_rotation() {
        assert_eq!(rotation_for_class(OrientModel::PpLcnet, 0), 0);
        assert_eq!(rotation_for_class(OrientModel::PpLcnet, 1), 270);
        assert_eq!(rotation_for_class(OrientModel::PpLcnet, 3), 90);
        assert_eq!(rotation_for_class(OrientModel::DoctrMobilenet, 1), 90);
    }

    #[test]
    fn inputs_have_model_shapes() {
        let img = Image::filled(300, 500, 3, 200);
        assert_eq!(pplcnet_input(&img).unwrap().shape, vec![1, 3, 224, 224]);
        assert_eq!(doctr_input(&img).unwrap().shape, vec![1, 3, 512, 512]);
        let a = area_resize(&Image::filled(10, 10, 1, 100), 5, 5);
        assert!(a.data.iter().all(|&v| v == 100));
    }
}
