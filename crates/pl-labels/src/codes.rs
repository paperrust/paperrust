//! Barcode symbols as a module grid (one module = one cell), generated with rxing (Apache-2.0,
//! the same library the consumer reads with). Drawing is vector-based (`draw`).

use serde::{Deserialize, Serialize};

use crate::{LabelError, Result};

/// Code kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeType {
    /// QR code (error correction M) – default, most robust.
    #[default]
    Qr,
    /// Data Matrix (ECC 200, square) – smallest.
    DataMatrix,
    /// Code 128 (1D) – for handheld/older scanner software.
    Code128,
}

impl CodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeType::Qr => "qr",
            CodeType::DataMatrix => "data_matrix",
            CodeType::Code128 => "code128",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "qr" => Some(CodeType::Qr),
            "data_matrix" | "datamatrix" => Some(CodeType::DataMatrix),
            "code128" => Some(CodeType::Code128),
            _ => None,
        }
    }

    /// zxing-cpp format name that the consumer reports when reading (`DocumentBarcode.Format`).
    pub fn zxing_name(self) -> &'static str {
        match self {
            CodeType::Qr => "QRCode",
            CodeType::DataMatrix => "DataMatrix",
            CodeType::Code128 => "Code128",
        }
    }

    pub fn is_1d(self) -> bool {
        matches!(self, CodeType::Code128)
    }

    /// Recommended quiet zone in modules (inside the label, in addition to the label margin).
    /// QR: standard 4, on labels with a white margin 2 is safely enough; DataMatrix: 1; Code 128: 10.
    pub fn quiet_modules(self) -> u32 {
        match self {
            CodeType::Qr => 2,
            CodeType::DataMatrix => 1,
            CodeType::Code128 => 10,
        }
    }
}

/// Module grid of a symbol (without quiet zone). For 1D codes `height == 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub kind: CodeType,
    pub width: u32,
    pub height: u32,
    /// zeilenweise, `true` = dunkel
    pub dark: Vec<bool>,
}

impl Symbol {
    pub fn get(&self, x: u32, y: u32) -> bool {
        x < self.width && y < self.height && self.dark[(y * self.width + x) as usize]
    }

    /// Dark runs per row: `(y, x_start, length)`.
    pub fn runs(&self) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        for y in 0..self.height {
            let mut x = 0;
            while x < self.width {
                if self.get(x, y) {
                    let s = x;
                    while x < self.width && self.get(x, y) {
                        x += 1;
                    }
                    out.push((y, s, x - s));
                } else {
                    x += 1;
                }
            }
        }
        out
    }
}

/// Encodes `content` as a symbol of kind `kind`.
pub fn encode(kind: CodeType, content: &str) -> Result<Symbol> {
    use rxing::Writer;
    if content.is_empty() {
        return Err(LabelError::Invalid("content".into(), "Barcode content must not be empty.".into()));
    }
    let hints = rxing::EncodeHints {
        Margin: Some("0".into()),
        ErrorCorrection: matches!(kind, CodeType::Qr).then(|| "M".to_string()),
        DataMatrixShape: matches!(kind, CodeType::DataMatrix).then_some(rxing::datamatrix::encoder::SymbolShapeHint::FORCE_SQUARE),
        ..Default::default()
    };
    let m = match kind {
        CodeType::Qr => rxing::qrcode::QRCodeWriter.encode_with_hints(content, &rxing::BarcodeFormat::QR_CODE, 0, 0, &hints),
        CodeType::DataMatrix => {
            rxing::datamatrix::DataMatrixWriter.encode_with_hints(content, &rxing::BarcodeFormat::DATA_MATRIX, 0, 0, &hints)
        }
        CodeType::Code128 => rxing::oned::Code128Writer.encode_with_hints(content, &rxing::BarcodeFormat::CODE_128, 0, 0, &hints),
    }
    .map_err(|e| LabelError::Invalid("content".into(), format!("Cannot encode {content:?} as {}: {e}", kind.as_str())))?;
    let (w, h) = (m.getWidth(), m.getHeight());
    let height = if kind.is_1d() { 1 } else { h };
    let mut dark = Vec::with_capacity((w * height) as usize);
    for y in 0..height {
        for x in 0..w {
            dark.push(m.get(x, y));
        }
    }
    let mut sym = Symbol { kind, width: w, height, dark };
    trim(&mut sym);
    Ok(sym)
}

/// Removes light margins (in case an encoder delivers quiet zones despite `Margin 0`).
fn trim(s: &mut Symbol) {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..s.height {
        for x in 0..s.width {
            if s.get(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == u32::MAX || (x0 == 0 && y0 == 0 && x1 + 1 == s.width && y1 + 1 == s.height) {
        return;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut dark = Vec::with_capacity((w * h) as usize);
    for y in y0..=y1 {
        for x in x0..=x1 {
            dark.push(s.get(x, y));
        }
    }
    s.width = w;
    s.height = h;
    s.dark = dark;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_have_expected_shape() {
        let q = encode(CodeType::Qr, "ASN00001").unwrap();
        assert_eq!((q.width, q.height), (21, 21), "version 1 QR");
        // Finder pattern top left: 7 dark modules in row 0
        assert!((0..7).all(|x| q.get(x, 0)));
        let d = encode(CodeType::DataMatrix, "ASN00001").unwrap();
        assert_eq!(d.width, d.height, "square DataMatrix");
        assert!(d.width <= 16);
        // L pattern: left column and bottom row fully dark
        assert!((0..d.height).all(|y| d.get(0, y)));
        assert!((0..d.width).all(|x| d.get(x, d.height - 1)));
        let c = encode(CodeType::Code128, "ASN00001").unwrap();
        assert_eq!(c.height, 1);
        assert!(c.get(0, 0) && c.get(c.width - 1, 0), "bars start and end dark");
        assert!(c.width > 60 && c.width < 130, "{}", c.width);
        assert!(encode(CodeType::Qr, "").is_err());
    }

    #[test]
    fn runs_cover_dark_modules() {
        let q = encode(CodeType::Qr, "PATCHT").unwrap();
        let n: u32 = q.runs().iter().map(|r| r.2).sum();
        assert_eq!(n as usize, q.dark.iter().filter(|d| **d).count());
    }

    #[test]
    fn parse_names() {
        for k in [CodeType::Qr, CodeType::DataMatrix, CodeType::Code128] {
            assert_eq!(CodeType::parse(k.as_str()), Some(k));
        }
        assert_eq!(serde_json::to_string(&CodeType::DataMatrix).unwrap(), "\"data_matrix\"");
    }
}
