//! Embedded "GlyphLessFont" for the invisible text layer (PDF/A: all fonts embedded).
//!
//! Model: Tesseract `pdfrenderer.cpp` / OCRmyPDF `hocrtransform` (`pdf.ttf`): a TrueType font
//! with two empty glyphs (GID 0 `.notdef`, GID 1 empty, advance 500/1000 em). Every CID is mapped via
//! a `CIDToGIDMap` stream to GID 1, the width in the PDF (`/DW 500`) matches the
//! advance width in the font program (PDF/A-1 6.3.6, PDF/A-2 6.2.11.5).
//!
//! The file is generated here itself (no foreign font file in the repository).

/// Units per em (widths in the PDF are in 1/1000 em → 1:1).
pub const UNITS_PER_EM: u16 = 1000;
/// Advance of every glyph (= `/DW` in the CIDFont).
pub const ADVANCE: u16 = 500;

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        sum = sum.wrapping_add(u32::from_be_bytes(b));
    }
    sum
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn head() -> Vec<u8> {
    let mut t = Vec::with_capacity(54);
    be32(&mut t, 0x0001_0000); // version
    be32(&mut t, 0x0001_0000); // fontRevision
    be32(&mut t, 0); // checkSumAdjustment (later)
    be32(&mut t, 0x5F0F_3CF5); // magicNumber
    be16(&mut t, 0x000B); // flags
    be16(&mut t, UNITS_PER_EM);
    t.extend_from_slice(&[0; 16]); // created, modified
    for v in [0i16, 0, ADVANCE as i16, UNITS_PER_EM as i16] {
        // xMin yMin xMax yMax
        be16(&mut t, v as u16);
    }
    be16(&mut t, 0); // macStyle
    be16(&mut t, 8); // lowestRecPPEM
    be16(&mut t, 2); // fontDirectionHint
    be16(&mut t, 0); // indexToLocFormat: kurz
    be16(&mut t, 0); // glyphDataFormat
    t
}

fn hhea() -> Vec<u8> {
    let mut t = Vec::with_capacity(36);
    be32(&mut t, 0x0001_0000);
    be16(&mut t, UNITS_PER_EM); // ascender
    be16(&mut t, 0); // descender
    be16(&mut t, 0); // lineGap
    be16(&mut t, ADVANCE); // advanceWidthMax
    be16(&mut t, 0); // minLeftSideBearing
    be16(&mut t, 0); // minRightSideBearing
    be16(&mut t, 0); // xMaxExtent
    be16(&mut t, 1); // caretSlopeRise
    be16(&mut t, 0); // caretSlopeRun
    be16(&mut t, 0); // caretOffset
    t.extend_from_slice(&[0; 8]); // reserved
    be16(&mut t, 0); // metricDataFormat
    be16(&mut t, 2); // numberOfHMetrics
    t
}

fn maxp() -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    be32(&mut t, 0x0001_0000);
    be16(&mut t, 2); // numGlyphs
    for v in [0u16, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut t, v);
    }
    t
}

fn hmtx() -> Vec<u8> {
    let mut t = Vec::new();
    for _ in 0..2 {
        be16(&mut t, ADVANCE);
        be16(&mut t, 0);
    }
    t
}

fn cmap() -> Vec<u8> {
    // (3,1) format 4 with only the end segment 0xFFFF (no characters; CIDs go via CIDToGIDMap).
    let mut sub = Vec::new();
    be16(&mut sub, 4); // format
    be16(&mut sub, 24); // length
    be16(&mut sub, 0); // language
    be16(&mut sub, 2); // segCountX2
    be16(&mut sub, 2); // searchRange
    be16(&mut sub, 0); // entrySelector
    be16(&mut sub, 0); // rangeShift
    be16(&mut sub, 0xFFFF); // endCode
    be16(&mut sub, 0); // reservedPad
    be16(&mut sub, 0xFFFF); // startCode
    be16(&mut sub, 1); // idDelta
    be16(&mut sub, 0); // idRangeOffset
    let mut t = Vec::new();
    be16(&mut t, 0); // version
    be16(&mut t, 1); // numTables
    be16(&mut t, 3);
    be16(&mut t, 1);
    be32(&mut t, 12);
    t.extend_from_slice(&sub);
    t
}

fn name() -> Vec<u8> {
    let records: [(u16, &str); 4] = [(1, "GlyphLessFont"), (2, "Regular"), (4, "GlyphLessFont"), (6, "GlyphLessFont")];
    let strings: Vec<Vec<u8>> = records.iter().map(|(_, s)| s.encode_utf16().flat_map(u16::to_be_bytes).collect()).collect();
    let mut t = Vec::new();
    be16(&mut t, 0); // format
    be16(&mut t, records.len() as u16);
    be16(&mut t, (6 + 12 * records.len()) as u16); // stringOffset
    let mut off = 0u16;
    for ((id, _), s) in records.iter().zip(&strings) {
        for v in [3u16, 1, 0x0409, *id, s.len() as u16, off] {
            be16(&mut t, v);
        }
        off += s.len() as u16;
    }
    for s in &strings {
        t.extend_from_slice(s);
    }
    t
}

fn post() -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    be32(&mut t, 0x0003_0000); // version 3: no glyph names
    be32(&mut t, 0); // italicAngle
    be16(&mut t, 0); // underlinePosition
    be16(&mut t, 0); // underlineThickness
    be32(&mut t, 1); // isFixedPitch
    t.extend_from_slice(&[0; 16]);
    t
}

/// TrueType file of the GlyphLessFont (≈ 400 bytes).
pub fn glyphless_ttf() -> Vec<u8> {
    // Both glyphs empty → loca entries 0; glyf with 4 padding bytes (some readers do not like
    // tables of length 0).
    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"cmap", cmap()),
        (*b"glyf", vec![0; 4]),
        (*b"head", head()),
        (*b"hhea", hhea()),
        (*b"hmtx", hmtx()),
        (*b"loca", vec![0; 6]),
        (*b"maxp", maxp()),
        (*b"name", name()),
        (*b"post", post()),
    ];
    tables.sort_by_key(|t| t.0);
    let n = tables.len() as u16;
    let mut pow = 1u16;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, n);
    be16(&mut out, pow * 16);
    be16(&mut out, log);
    be16(&mut out, n * 16 - pow * 16);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_off = 0;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        be32(&mut out, checksum(data));
        be32(&mut out, offset as u32);
        be32(&mut out, data.len() as u32);
        if tag == b"head" {
            head_off = offset;
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_off + 8..head_off + 12].copy_from_slice(&adj.to_be_bytes());
    out
}

/// `CIDToGIDMap` stream: CID 0 → GID 0, CID 1..=max → GID 1.
pub fn cid_to_gid_map(max_cid: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity((usize::from(max_cid) + 1) * 2);
    v.extend_from_slice(&[0, 0]);
    for _ in 0..max_cid {
        v.extend_from_slice(&[0, 1]);
    }
    v
}

/// Small TrueType reader for the self-check (table directory, numGlyphs, advances).
pub mod read {
    pub fn table<'a>(ttf: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
        let n = usize::from(u16::from_be_bytes([*ttf.get(4)?, *ttf.get(5)?]));
        for i in 0..n {
            let e = ttf.get(12 + 16 * i..28 + 16 * i)?;
            if &e[..4] == tag {
                let off = u32::from_be_bytes([e[8], e[9], e[10], e[11]]) as usize;
                let len = u32::from_be_bytes([e[12], e[13], e[14], e[15]]) as usize;
                return ttf.get(off..off.checked_add(len)?);
            }
        }
        None
    }

    pub fn num_glyphs(ttf: &[u8]) -> Option<u16> {
        let m = table(ttf, b"maxp")?;
        Some(u16::from_be_bytes([*m.get(4)?, *m.get(5)?]))
    }

    pub fn units_per_em(ttf: &[u8]) -> Option<u16> {
        let h = table(ttf, b"head")?;
        Some(u16::from_be_bytes([*h.get(18)?, *h.get(19)?]))
    }

    /// Advance of glyph `gid` in font units.
    pub fn advance(ttf: &[u8], gid: u16) -> Option<u16> {
        let hh = table(ttf, b"hhea")?;
        let n_h = u16::from_be_bytes([*hh.get(34)?, *hh.get(35)?]);
        let hm = table(ttf, b"hmtx")?;
        let i = usize::from(gid.min(n_h.saturating_sub(1)));
        Some(u16::from_be_bytes([*hm.get(4 * i)?, *hm.get(4 * i + 1)?]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttf_is_well_formed() {
        let f = glyphless_ttf();
        assert_eq!(&f[..4], &[0, 1, 0, 0]);
        assert_eq!(f.len() % 4, 0);
        // total checksum with checkSumAdjustment = 0xB1B0AFBA
        assert_eq!(checksum(&f), 0xB1B0_AFBA);
        assert_eq!(read::num_glyphs(&f), Some(2));
        assert_eq!(read::units_per_em(&f), Some(UNITS_PER_EM));
        assert_eq!(read::advance(&f, 1), Some(ADVANCE));
        assert_eq!(read::advance(&f, 7), Some(ADVANCE));
        for tag in [b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp"] {
            assert!(read::table(&f, tag).is_some(), "{}", String::from_utf8_lossy(tag));
        }
        // ab_glyph (dev-dependency) can load the file
        let font = ab_glyph::FontRef::try_from_slice(&f).expect("parse");
        use ab_glyph::Font;
        assert_eq!(font.glyph_count(), 2);
    }

    #[test]
    fn cid_map() {
        assert_eq!(cid_to_gid_map(2), vec![0, 0, 0, 1, 0, 1]);
    }
}
