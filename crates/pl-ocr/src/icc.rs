//! Self-generated sRGB ICC profile (ICC v2.1, monitor class, matrix/TRC) for the PDF/A
//! OutputIntent. upstream uses `ocrmypdf/data/sRGB.icc`; here an equivalent profile is built from
//! the sRGB primaries (D50-adapted, IEC 61966-2-1), so that no foreign profile file
//! has to be shipped.

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    t.extend_from_slice(&s15f16(x));
    t.extend_from_slice(&s15f16(y));
    t.extend_from_slice(&s15f16(z));
    t
}

fn desc_tag(s: &str) -> Vec<u8> {
    let mut t = b"desc\0\0\0\0".to_vec();
    t.extend_from_slice(&((s.len() + 1) as u32).to_be_bytes());
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t.extend_from_slice(&[0; 4]); // Unicode language code
    t.extend_from_slice(&[0; 4]); // Unicode length
    t.extend_from_slice(&[0; 2]); // ScriptCode code
    t.push(0); // ScriptCode length
    t.extend_from_slice(&[0; 67]);
    t
}

fn text_tag(s: &str) -> Vec<u8> {
    let mut t = b"text\0\0\0\0".to_vec();
    t.extend_from_slice(s.as_bytes());
    t.push(0);
    t
}

fn srgb_curve() -> Vec<u8> {
    const N: usize = 1024;
    let mut t = b"curv\0\0\0\0".to_vec();
    t.extend_from_slice(&(N as u32).to_be_bytes());
    for i in 0..N {
        let v = i as f64 / (N - 1) as f64;
        let lin = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        t.extend_from_slice(&((lin * 65535.0).round() as u16).to_be_bytes());
    }
    t
}

/// sRGB profile (≈ 2.6 KB).
pub fn srgb_profile() -> Vec<u8> {
    let curve = srgb_curve();
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag("sRGB IEC61966-2.1 (PaperRust)")),
        (b"cprt", text_tag("No copyright, use freely")),
        (b"wtpt", xyz_tag(0.9642, 1.0, 0.8249)),
        (b"rXYZ", xyz_tag(0.4360, 0.2225, 0.0139)),
        (b"gXYZ", xyz_tag(0.3851, 0.7169, 0.0971)),
        (b"bXYZ", xyz_tag(0.1431, 0.0606, 0.7141)),
        (b"rTRC", curve),
    ];
    // gTRC/bTRC share the data of rTRC (ICC allows shared tag data).
    build_profile(b"RGB ", &tags, Some(b"rTRC"), &[*b"gTRC", *b"bTRC"])
}

/// Grayscale profile (ICC v2.1, monitor class, `kTRC` = sRGB curve, white point D50) for
/// `UseDeviceIndependentColor` (gray images as `ICCBased` with N 1).
pub fn gray_profile() -> Vec<u8> {
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag("Gray sRGB-TRC (PaperRust)")),
        (b"cprt", text_tag("No copyright, use freely")),
        (b"wtpt", xyz_tag(0.9642, 1.0, 0.8249)),
        (b"kTRC", srgb_curve()),
    ];
    build_profile(b"GRAY", &tags, None, &[])
}

/// Assemble the profile; `shared`: further tag signatures that share the data of `share_from`.
fn build_profile(color_space: &[u8; 4], tags: &[(&[u8; 4], Vec<u8>)], share_from: Option<&[u8; 4]>, shared: &[[u8; 4]]) -> Vec<u8> {
    let n_entries = tags.len() + shared.len();
    let table_len = 4 + 12 * n_entries;
    let mut data = Vec::new();
    let mut entries = Vec::new();
    let mut offset = 128 + table_len;
    let mut trc = (0, 0);
    for (sig, body) in tags {
        while !offset.is_multiple_of(4) {
            offset += 1;
            data.push(0);
        }
        entries.push((**sig, offset as u32, body.len() as u32));
        if Some(*sig) == share_from {
            trc = (offset as u32, body.len() as u32);
        }
        offset += body.len();
        data.extend_from_slice(body);
    }
    for sig in shared {
        entries.push((*sig, trc.0, trc.1));
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let total = 128 + table_len + data.len();

    let mut h = Vec::with_capacity(total);
    h.extend_from_slice(&(total as u32).to_be_bytes());
    h.extend_from_slice(b"\0\0\0\0"); // CMM
    h.extend_from_slice(&[0x02, 0x10, 0, 0]); // Version 2.1
    h.extend_from_slice(b"mntr");
    h.extend_from_slice(color_space);
    h.extend_from_slice(b"XYZ ");
    for v in [2026u16, 1, 1, 0, 0, 0] {
        h.extend_from_slice(&v.to_be_bytes());
    }
    h.extend_from_slice(b"acsp");
    h.extend_from_slice(&[0; 4]); // platform
    h.extend_from_slice(&[0; 4]); // Flags
    h.extend_from_slice(&[0; 4]); // manufacturer
    h.extend_from_slice(&[0; 4]); // model
    h.extend_from_slice(&[0; 8]); // Attribute
    h.extend_from_slice(&[0; 4]); // Rendering Intent: perceptual
    h.extend_from_slice(&s15f16(0.9642));
    h.extend_from_slice(&s15f16(1.0));
    h.extend_from_slice(&s15f16(0.8249));
    h.extend_from_slice(&[0; 4]); // Creator
    h.extend_from_slice(&[0; 16]); // profile ID
    h.extend_from_slice(&[0; 28]);
    debug_assert_eq!(h.len(), 128);
    h.extend_from_slice(&(n_entries as u32).to_be_bytes());
    for (sig, off, len) in entries {
        h.extend_from_slice(&sig);
        h.extend_from_slice(&off.to_be_bytes());
        h.extend_from_slice(&len.to_be_bytes());
    }
    h.extend_from_slice(&data);
    h
}

#[cfg(test)]
mod tests {
    #[test]
    fn header() {
        let p = super::srgb_profile();
        assert_eq!(u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize, p.len());
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[16..20], b"RGB ");
        assert_eq!(p.len() % 4, 0);
        let g = super::gray_profile();
        assert_eq!(u32::from_be_bytes([g[0], g[1], g[2], g[3]]) as usize, g.len());
        assert_eq!(&g[16..20], b"GRAY");
        assert_eq!(g[8], 2);
    }
}
