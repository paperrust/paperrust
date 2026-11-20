//! Minimal TrueType reader and subsetter for substitute fonts (gap E3).
//!
//! Subset by "emptying": all glyph numbers stay, unneeded glyphs become empty
//! (composite glyphs keep their components). Widths (`hmtx`) can be overridden per glyph
//! so that they match `/Widths` in the PDF (PDF/A 6.2.11.5). Tables that depend on
//! widths or glyph numbers and are not needed for PDF (`GSUB`, `GPOS`, `GDEF`,
//! `kern`, `hdmx`, `LTSH`, `VDMX`, `DSIG`, …) are dropped.

use std::collections::{BTreeMap, BTreeSet};

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn i16_at(b: &[u8], o: usize) -> Option<i16> {
    u16_at(b, o).map(|v| v as i16)
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(o)?, *b.get(o + 1)?, *b.get(o + 2)?, *b.get(o + 3)?]))
}

/// Opened TrueType font (`glyf` fonts only, no CFF/OTF).
pub struct Ttf<'a> {
    data: &'a [u8],
    tables: BTreeMap<[u8; 4], (usize, usize)>,
    pub units_per_em: u16,
    pub bbox: [i16; 4],
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    pub italic_angle: f64,
    pub num_glyphs: u16,
    long_loca: bool,
    num_hmetrics: u16,
    /// (3,1)/(3,10)/(0,x) Unicode cmap: offset of the subtable.
    cmap: Option<usize>,
}

impl<'a> Ttf<'a> {
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let ver = u32_at(data, 0)?;
        if ver != 0x0001_0000 && ver != u32::from_be_bytes(*b"true") {
            return None;
        }
        let n = usize::from(u16_at(data, 4)?);
        let mut tables = BTreeMap::new();
        for i in 0..n {
            let r = 12 + i * 16;
            let tag: [u8; 4] = data.get(r..r + 4)?.try_into().ok()?;
            let off = u32_at(data, r + 8)? as usize;
            let len = u32_at(data, r + 12)? as usize;
            data.get(off..off.checked_add(len)?)?;
            tables.insert(tag, (off, len));
        }
        let t = |tag: &[u8; 4]| tables.get(tag).copied();
        let (head, _) = t(b"head")?;
        let (hhea, _) = t(b"hhea")?;
        let (maxp, _) = t(b"maxp")?;
        t(b"glyf")?;
        t(b"loca")?;
        t(b"hmtx")?;
        let italic_angle = t(b"post").and_then(|(o, _)| u32_at(data, o + 4)).map_or(0.0, |v| f64::from(v as i32) / 65536.0);
        let ascent = i16_at(data, hhea + 4)?;
        let cap_height = t(b"OS/2")
            .filter(|(o, l)| *l >= 90 && u16_at(data, *o).is_some_and(|v| v >= 2))
            .and_then(|(o, _)| i16_at(data, o + 88))
            .unwrap_or(ascent);
        let mut f = Self {
            data,
            units_per_em: u16_at(data, head + 18)?.max(16),
            bbox: [i16_at(data, head + 36)?, i16_at(data, head + 38)?, i16_at(data, head + 40)?, i16_at(data, head + 42)?],
            ascent,
            descent: i16_at(data, hhea + 6)?,
            cap_height,
            italic_angle,
            num_glyphs: u16_at(data, maxp + 4)?,
            long_loca: i16_at(data, head + 50)? == 1,
            num_hmetrics: u16_at(data, hhea + 34)?.max(1),
            cmap: None,
            tables,
        };
        f.cmap = f.find_cmap();
        f.cmap?;
        Some(f)
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        let (o, l) = *self.tables.get(tag)?;
        self.data.get(o..o + l)
    }

    fn find_cmap(&self) -> Option<usize> {
        let (base, _) = *self.tables.get(b"cmap")?;
        let n = usize::from(u16_at(self.data, base + 2)?);
        let mut best: Option<(u8, usize)> = None;
        for i in 0..n {
            let r = base + 4 + i * 8;
            let (pid, eid) = (u16_at(self.data, r)?, u16_at(self.data, r + 2)?);
            let off = base + u32_at(self.data, r + 4)? as usize;
            let fmt = u16_at(self.data, off)?;
            let rank = match (pid, eid, fmt) {
                (3, 10, 12) => 4,
                (3, 1, 4) => 3,
                (0, _, 12) => 2,
                (0, _, 4) => 1,
                _ => continue,
            };
            if best.is_none_or(|(b, _)| rank > b) {
                best = Some((rank, off));
            }
        }
        best.map(|(_, o)| o)
    }

    /// Glyph for a Unicode character (0 = missing).
    pub fn glyph(&self, c: u32) -> u16 {
        self.glyph_opt(c).unwrap_or(0)
    }

    fn glyph_opt(&self, c: u32) -> Option<u16> {
        let d = self.data;
        let o = self.cmap?;
        match u16_at(d, o)? {
            4 => {
                let c = u16::try_from(c).ok()?;
                let seg2 = usize::from(u16_at(d, o + 6)?);
                let ends = o + 14;
                let starts = ends + seg2 + 2;
                let deltas = starts + seg2;
                let ranges = deltas + seg2;
                for i in (0..seg2).step_by(2) {
                    let end = u16_at(d, ends + i)?;
                    if c > end {
                        continue;
                    }
                    let start = u16_at(d, starts + i)?;
                    if c < start {
                        return None;
                    }
                    let delta = u16_at(d, deltas + i)?;
                    let ro = usize::from(u16_at(d, ranges + i)?);
                    if ro == 0 {
                        return Some(c.wrapping_add(delta));
                    }
                    let g = u16_at(d, ranges + i + ro + 2 * usize::from(c - start))?;
                    return (g != 0).then(|| g.wrapping_add(delta));
                }
                None
            }
            12 => {
                let n = u32_at(d, o + 12)? as usize;
                for i in 0..n.min(100_000) {
                    let g = o + 16 + i * 12;
                    let (s, e, id) = (u32_at(d, g)?, u32_at(d, g + 4)?, u32_at(d, g + 8)?);
                    if (s..=e).contains(&c) {
                        return u16::try_from(id + (c - s)).ok();
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Advance of a glyph in font units.
    pub fn advance(&self, g: u16) -> u16 {
        let Some(h) = self.table(b"hmtx") else { return 0 };
        let i = usize::from(g.min(self.num_hmetrics - 1));
        u16_at(h, i * 4).unwrap_or(0)
    }

    fn lsb(&self, g: u16) -> i16 {
        let Some(h) = self.table(b"hmtx") else { return 0 };
        let nh = usize::from(self.num_hmetrics);
        let g = usize::from(g);
        if g < nh { i16_at(h, g * 4 + 2) } else { i16_at(h, nh * 4 + (g - nh) * 2) }.unwrap_or(0)
    }

    fn glyph_range(&self, g: u16) -> Option<(usize, usize)> {
        let loca = self.table(b"loca")?;
        let g = usize::from(g);
        let (a, b) = if self.long_loca {
            (u32_at(loca, g * 4)? as usize, u32_at(loca, g * 4 + 4)? as usize)
        } else {
            (usize::from(u16_at(loca, g * 2)?) * 2, usize::from(u16_at(loca, g * 2 + 2)?) * 2)
        };
        (b >= a).then_some((a, b))
    }

    /// Components of a composite glyph.
    fn components(&self, g: u16) -> Vec<u16> {
        let mut out = Vec::new();
        let (Some(glyf), Some((a, b))) = (self.table(b"glyf"), self.glyph_range(g)) else { return out };
        let Some(gd) = glyf.get(a..b) else { return out };
        if gd.len() < 10 || i16_at(gd, 0).unwrap_or(0) >= 0 {
            return out;
        }
        let mut p = 10usize;
        while let (Some(flags), Some(idx)) = (u16_at(gd, p), u16_at(gd, p + 2)) {
            out.push(idx);
            p += 4 + if flags & 1 != 0 { 4 } else { 2 };
            p += if flags & 0x8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            if flags & 0x20 == 0 || out.len() > 64 {
                break;
            }
        }
        out
    }

    /// Subset: keep only `keep` (plus .notdef and components); `widths` overrides
    /// advances (glyph → font units).
    pub fn subset(&self, keep: &BTreeSet<u16>, widths: &BTreeMap<u16, u16>) -> Option<Vec<u8>> {
        let mut all: BTreeSet<u16> = keep.iter().copied().filter(|g| *g < self.num_glyphs).collect();
        all.insert(0);
        let mut todo: Vec<u16> = all.iter().copied().collect();
        while let Some(g) = todo.pop() {
            for c in self.components(g) {
                if c < self.num_glyphs && all.insert(c) {
                    todo.push(c);
                }
            }
        }
        let glyf = self.table(b"glyf")?;
        let mut new_glyf = Vec::new();
        let mut loca = Vec::with_capacity((usize::from(self.num_glyphs) + 1) * 4);
        for g in 0..self.num_glyphs {
            loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
            if all.contains(&g)
                && let Some((a, b)) = self.glyph_range(g)
            {
                new_glyf.extend_from_slice(glyf.get(a..b)?);
                while new_glyf.len() % 4 != 0 {
                    new_glyf.push(0);
                }
            }
        }
        loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        let mut hmtx = Vec::with_capacity(usize::from(self.num_glyphs) * 4);
        let mut adv_max = 0u16;
        for g in 0..self.num_glyphs {
            let adv = widths.get(&g).copied().unwrap_or_else(|| self.advance(g));
            adv_max = adv_max.max(adv);
            hmtx.extend_from_slice(&adv.to_be_bytes());
            hmtx.extend_from_slice(&self.lsb(g).to_be_bytes());
        }
        let mut head = self.table(b"head")?.to_vec();
        head.get_mut(8..12)?.fill(0);
        head.get_mut(50..52)?.copy_from_slice(&1i16.to_be_bytes());
        let mut hhea = self.table(b"hhea")?.to_vec();
        hhea.get_mut(10..12)?.copy_from_slice(&adv_max.to_be_bytes());
        hhea.get_mut(34..36)?.copy_from_slice(&self.num_glyphs.to_be_bytes());
        let mut out_tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"glyf", new_glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
        ];
        for tag in [b"cmap", b"cvt ", b"fpgm", b"gasp", b"maxp", b"name", b"OS/2", b"post", b"prep"] {
            if let Some(t) = self.table(tag) {
                out_tables.push((*tag, t.to_vec()));
            }
        }
        out_tables.sort_by_key(|(t, _)| *t);
        Some(write_sfnt(&out_tables))
    }
}

fn checksum(b: &[u8]) -> u32 {
    b.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    })
}

fn write_sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let n = tables.len() as u16;
    let mut es = 0u16;
    while (1u32 << (es + 1)) <= u32::from(n) {
        es += 1;
    }
    let sr = (1u16 << es) * 16;
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&sr.to_be_bytes());
    out.extend_from_slice(&es.to_be_bytes());
    out.extend_from_slice(&(n * 16 - sr).to_be_bytes());
    let mut off = 12 + tables.len() * 16;
    let mut head_off = None;
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_off = Some(off);
        }
        off += data.len().div_ceil(4) * 4;
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(h) = head_off {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        if let Some(s) = out.get_mut(h + 8..h + 12) {
            s.copy_from_slice(&adj.to_be_bytes());
        }
    }
    out
}
