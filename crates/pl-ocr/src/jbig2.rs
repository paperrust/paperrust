//! Lossless JBIG2 encoder (generic region, MQ arithmetic coder) in pure Rust –
//! replacement for `jbig2enc` (OCRmyPDF `--optimize 1..3` encodes 1-bit images as JBIG2).
//!
//! Scope: one page = one "immediate generic region" (ITU-T T.88 §6.2, template 0, default
//! AT pixels, without TPGDON/MMR), embedded organization for PDF (`/Filter /JBIG2Decode`, without
//! file header, end-of-page and end-of-file, PDF 32000-1 §7.4.7). No symbol dictionary (that would be
//! "lossy"/`jbig2_lossy`, deprecated in OCRmyPDF 17 anyway).
//!
//! Pixel convention: JBIG2 1 = black. In PDF `JBIG2Decode` returns 0 for black, i.e. the
//! packed 1-bit image (DeviceGray, 0 = black) is inverted when encoding.

/// Qe-Tabelle (T.88 Tabelle E.1): (Qe, NMPS, NLPS, SWITCH).
const QE: [(u32, u8, u8, bool); 47] = [
    (0x5601, 1, 1, true),
    (0x3401, 2, 6, false),
    (0x1801, 3, 9, false),
    (0x0AC1, 4, 12, false),
    (0x0521, 5, 29, false),
    (0x0221, 38, 33, false),
    (0x5601, 7, 6, true),
    (0x5401, 8, 14, false),
    (0x4801, 9, 14, false),
    (0x3801, 10, 14, false),
    (0x3001, 11, 17, false),
    (0x2401, 12, 18, false),
    (0x1C01, 13, 20, false),
    (0x1601, 29, 21, false),
    (0x5601, 15, 14, true),
    (0x5401, 16, 14, false),
    (0x5101, 17, 15, false),
    (0x4801, 18, 16, false),
    (0x3801, 19, 17, false),
    (0x3401, 20, 18, false),
    (0x3001, 21, 19, false),
    (0x2801, 22, 19, false),
    (0x2401, 23, 20, false),
    (0x2201, 24, 21, false),
    (0x1C01, 25, 22, false),
    (0x1801, 26, 23, false),
    (0x1601, 27, 24, false),
    (0x1401, 28, 25, false),
    (0x1201, 29, 26, false),
    (0x1101, 30, 27, false),
    (0x0AC1, 31, 28, false),
    (0x09C1, 32, 29, false),
    (0x08A1, 33, 30, false),
    (0x0521, 34, 31, false),
    (0x0441, 35, 32, false),
    (0x02A1, 36, 33, false),
    (0x0221, 37, 34, false),
    (0x0141, 38, 35, false),
    (0x0111, 39, 36, false),
    (0x0085, 40, 37, false),
    (0x0049, 41, 38, false),
    (0x0025, 42, 39, false),
    (0x0015, 43, 40, false),
    (0x0009, 44, 41, false),
    (0x0005, 45, 42, false),
    (0x0001, 45, 43, false),
    (0x5601, 46, 46, false),
];

/// MQ encoder (T.88 Annex E.2, byte sequence like jbig2enc).
struct Mq {
    c: u32,
    a: u32,
    ct: u32,
    b: u8,
    started: bool,
    out: Vec<u8>,
    /// State per context: index (bits 0..6), MPS (bit 7).
    cx: Vec<u8>,
}

impl Mq {
    fn new(contexts: usize) -> Self {
        Mq { c: 0, a: 0x8000, ct: 12, b: 0, started: false, out: Vec::new(), cx: vec![0; contexts] }
    }

    fn emit(&mut self) {
        if self.started {
            self.out.push(self.b);
        }
        self.started = true;
    }

    fn byteout(&mut self) {
        if self.b == 0xFF {
            self.rblock();
            return;
        }
        if self.c < 0x800_0000 {
            self.lblock();
            return;
        }
        self.b = self.b.wrapping_add(1);
        if self.b != 0xFF {
            self.lblock();
            return;
        }
        self.c &= 0x7FF_FFFF;
        self.rblock();
    }

    fn rblock(&mut self) {
        self.emit();
        self.b = ((self.c >> 20) & 0xFF) as u8;
        self.c &= 0xF_FFFF;
        self.ct = 7;
    }

    fn lblock(&mut self) {
        self.emit();
        self.b = ((self.c >> 19) & 0xFF) as u8;
        self.c &= 0x7_FFFF;
        self.ct = 8;
    }

    fn renorm(&mut self) {
        loop {
            self.a <<= 1;
            self.c <<= 1;
            self.ct -= 1;
            if self.ct == 0 {
                self.byteout();
            }
            if self.a & 0x8000 != 0 {
                break;
            }
        }
    }

    fn encode(&mut self, ctx: usize, d: u8) {
        let st = self.cx[ctx];
        let idx = usize::from(st & 0x7F);
        let mps = st >> 7;
        let (qe, nmps, nlps, switch) = QE[idx];
        if d == mps {
            self.a -= qe;
            if self.a & 0x8000 == 0 {
                if self.a < qe {
                    self.a = qe;
                } else {
                    self.c += qe;
                }
                self.cx[ctx] = nmps | (mps << 7);
                self.renorm();
            } else {
                self.c += qe;
            }
        } else {
            self.a -= qe;
            if self.a < qe {
                self.c += qe;
            } else {
                self.a = qe;
            }
            let new_mps = if switch { 1 - mps } else { mps };
            self.cx[ctx] = nlps | (new_mps << 7);
            self.renorm();
        }
    }

    fn finish(mut self) -> Vec<u8> {
        let tempc = self.c + self.a;
        self.c |= 0xFFFF;
        if self.c >= tempc {
            self.c -= 0x8000;
        }
        self.c <<= self.ct;
        self.byteout();
        self.c <<= self.ct;
        self.byteout();
        self.emit();
        if self.b != 0xFF {
            self.b = 0xFF;
            self.emit();
        }
        self.b = 0xAC;
        self.emit();
        self.out
    }
}

fn segment_header(out: &mut Vec<u8>, number: u32, seg_type: u8, data_len: u32) {
    out.extend_from_slice(&number.to_be_bytes());
    out.push(seg_type & 0x3F); // page association 1 byte
    out.push(0); // no referred-to segments
    out.push(1); // page 1
    out.extend_from_slice(&data_len.to_be_bytes());
}

/// Packed 1-bit image (rows padded to bytes, MSB first, PDF convention 0 = black)
/// → JBIG2 data stream for `/JBIG2Decode`.
pub fn encode_generic(packed: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    let stride = w.div_ceil(8);
    if w == 0 || h == 0 || packed.len() < stride * h {
        return None;
    }
    // Pixel (x, y) as a JBIG2 value (1 = black), 0 outside.
    let px = |x: isize, y: isize| -> u32 {
        if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            return 0;
        }
        let (x, y) = (x as usize, y as usize);
        let bit = (packed[y * stride + x / 8] >> (7 - (x % 8))) & 1;
        u32::from(bit ^ 1)
    };
    let mut mq = Mq::new(1 << 16);
    for y in 0..h as isize {
        for x in 0..w as isize {
            // context template 0 (bit order like T.88 Fig. 3 / pdfium `decode_Arith_Template0`)
            let ctx = px(x - 1, y)
                | px(x - 2, y) << 1
                | px(x - 3, y) << 2
                | px(x - 4, y) << 3
                | px(x + 3, y - 1) << 4
                | px(x + 2, y - 1) << 5
                | px(x + 1, y - 1) << 6
                | px(x, y - 1) << 7
                | px(x - 1, y - 1) << 8
                | px(x - 2, y - 1) << 9
                | px(x - 3, y - 1) << 10
                | px(x + 2, y - 2) << 11
                | px(x + 1, y - 2) << 12
                | px(x, y - 2) << 13
                | px(x - 1, y - 2) << 14
                | px(x - 2, y - 2) << 15;
            mq.encode(ctx as usize, px(x, y) as u8);
        }
    }
    let coded = mq.finish();

    let mut out = Vec::with_capacity(coded.len() + 64);
    // page information (type 48)
    segment_header(&mut out, 0, 48, 19);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // X resolution unknown
    out.extend_from_slice(&0u32.to_be_bytes());
    out.push(0x01); // lossless coding, default pixel 0 (white), combination OR
    out.extend_from_slice(&0u16.to_be_bytes()); // no stripes
    // Immediate generic region (type 38): region info 17 + flags 1 + AT 8 + data
    let data_len = 17 + 1 + 8 + coded.len();
    segment_header(&mut out, 1, 38, u32::try_from(data_len).ok()?);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // x
    out.extend_from_slice(&0u32.to_be_bytes()); // y
    out.push(0); // combination OR
    out.push(0); // MMR 0, GBTEMPLATE 0, TPGDON 0
    for v in [3i8, -1, -3, -1, 2, -2, -2, -2] {
        out.push(v as u8);
    }
    out.extend_from_slice(&coded);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MQ decoder (T.88 E.3) only to verify the encoder.
    struct MqDec<'a> {
        data: &'a [u8],
        bp: usize,
        c: u32,
        a: u32,
        ct: i32,
        cx: Vec<u8>,
    }
    impl<'a> MqDec<'a> {
        fn byte(&self, i: usize) -> u32 {
            u32::from(*self.data.get(i).unwrap_or(&0xFF))
        }
        fn new(data: &'a [u8], n: usize) -> Self {
            let mut d = MqDec { data, bp: 0, c: 0, a: 0, ct: 0, cx: vec![0; n] };
            d.c = (d.byte(0) ^ 0xFF) << 16;
            d.bytein();
            d.c <<= 7;
            d.ct -= 7;
            d.a = 0x8000;
            d
        }
        fn bytein(&mut self) {
            if self.byte(self.bp) == 0xFF {
                if self.byte(self.bp + 1) > 0x8F {
                    self.ct = 8;
                } else {
                    self.bp += 1;
                    self.c = self.c.wrapping_add(0xFE00).wrapping_sub(self.byte(self.bp) << 9);
                    self.ct = 7;
                }
            } else {
                self.bp += 1;
                self.c = self.c.wrapping_add(0xFF00).wrapping_sub(self.byte(self.bp) << 8);
                self.ct = 8;
            }
        }
        fn renorm(&mut self) {
            loop {
                if self.ct == 0 {
                    self.bytein();
                }
                self.a <<= 1;
                self.c <<= 1;
                self.ct -= 1;
                if self.a & 0x8000 != 0 {
                    break;
                }
            }
        }
        fn decode(&mut self, ctx: usize) -> u8 {
            let st = self.cx[ctx];
            let idx = usize::from(st & 0x7F);
            let mps = st >> 7;
            let (qe, nmps, nlps, switch) = QE[idx];
            self.a -= qe;
            let d;
            if (self.c >> 16) < self.a {
                if self.a & 0x8000 == 0 {
                    // MPS exchange
                    if self.a < qe {
                        d = 1 - mps;
                        self.cx[ctx] = nlps | ((if switch { 1 - mps } else { mps }) << 7);
                    } else {
                        d = mps;
                        self.cx[ctx] = nmps | (mps << 7);
                    }
                    self.renorm();
                } else {
                    d = mps;
                }
            } else {
                self.c -= self.a << 16;
                if self.a < qe {
                    d = mps;
                    self.cx[ctx] = nmps | (mps << 7);
                } else {
                    d = 1 - mps;
                    self.cx[ctx] = nlps | ((if switch { 1 - mps } else { mps }) << 7);
                }
                self.a = qe;
                self.renorm();
            }
            d
        }
    }

    #[test]
    fn mq_roundtrip() {
        let mut rng = 12345u64;
        let mut bits = Vec::new();
        for i in 0..20_000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            // skewed distribution, two contexts
            bits.push((i % 2, u8::from(rng.is_multiple_of(10))));
        }
        let mut e = Mq::new(2);
        for (c, b) in &bits {
            e.encode(*c, *b);
        }
        let data = e.finish();
        assert!(data.len() < 20_000 / 8);
        let mut d = MqDec::new(&data, 2);
        for (i, (c, b)) in bits.iter().enumerate() {
            assert_eq!(d.decode(*c), *b, "bit {i}");
        }
    }

    #[test]
    fn segments_layout() {
        // 16x2, first row black (0 in PDF)
        let packed = vec![0x00, 0x00, 0xFF, 0xFF];
        let s = encode_generic(&packed, 16, 2).unwrap();
        assert_eq!(&s[0..4], &[0, 0, 0, 0]);
        assert_eq!(s[4], 48);
        assert_eq!(&s[7..11], &19u32.to_be_bytes());
        let g = 11 + 19;
        assert_eq!(&s[g..g + 4], &[0, 0, 0, 1]);
        assert_eq!(s[g + 4], 38);
        let len = u32::from_be_bytes([s[g + 7], s[g + 8], s[g + 9], s[g + 10]]) as usize;
        assert_eq!(s.len(), g + 11 + len);
        assert!(s.ends_with(&[0xFF, 0xAC]));
    }
}
