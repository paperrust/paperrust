//! Standard 14 fonts (Helvetica, Helvetica-Bold) without embedding: advance widths from the Adobe AFM
//! (1/1000 em) for centring and fitting, WinAnsi encoding (Latin-1 subset is enough for
//! labels and the de/en texts of the separator sheets).

/// Font style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    Regular,
    Bold,
}

impl Face {
    /// PDF resource name.
    pub fn resource(self) -> &'static str {
        match self {
            Face::Regular => "F1",
            Face::Bold => "F2",
        }
    }
}

/// Cap height (AFM `CapHeight`) in em.
pub const CAP_HEIGHT: f64 = 0.718;

// ASCII 32..=126, Helvetica.afm
const REGULAR: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ␠ .. /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // : ; < = > ? @
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667,
    611, // A-Z
    278, 278, 278, 469, 556, 333, // [ \ ] ^ _ `
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
    500, // a-z
    334, 260, 334, 584, // { | } ~
];

// ASCII 32..=126, Helvetica-Bold.afm
const BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, // ␠ .. /
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    333, 333, 584, 584, 584, 611, 975, // : ; < = > ? @
    722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667,
    611, // A-Z
    333, 278, 333, 584, 556, 333, // [ \ ] ^ _ `
    556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556,
    500, // a-z
    389, 280, 389, 584, // { | } ~
];

/// Base letter for advance widths of Latin-1 characters (accents do not change the width).
fn base_char(c: char) -> char {
    match c {
        'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' => 'a',
        'Ä' | 'À' | 'Á' | 'Â' | 'Ã' | 'Å' => 'A',
        'ö' | 'ò' | 'ó' | 'ô' | 'õ' => 'o',
        'Ö' | 'Ò' | 'Ó' | 'Ô' | 'Õ' => 'O',
        'ü' | 'ù' | 'ú' | 'û' => 'u',
        'Ü' | 'Ù' | 'Ú' | 'Û' => 'U',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'ß' => 'B',
        '–' | '—' => 'n',
        '…' => 'm',
        '’' | '‘' => '\'',
        '“' | '”' | '„' => '"',
        '·' | '•' => '.',
        _ => c,
    }
}

/// Laufweite eines Zeichens in 1/1000 em.
pub fn char_width(face: Face, c: char) -> u16 {
    let c = base_char(c);
    let table = match face {
        Face::Regular => &REGULAR,
        Face::Bold => &BOLD,
    };
    match c as u32 {
        cp @ 32..=126 => table[(cp - 32) as usize],
        _ => 556,
    }
}

/// Textbreite in em.
pub fn text_width_em(face: Face, s: &str) -> f64 {
    s.chars().map(|c| f64::from(char_width(face, c))).sum::<f64>() / 1000.0
}

/// One character → WinAnsi byte (`?` for non-representable ones).
pub fn winansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '€' => 0x80,
        '‚' => 0x82,
        '„' => 0x84,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        _ => b'?',
    }
}

/// PDF string `( … )` in WinAnsi with escapes.
pub fn pdf_string(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'(');
    for c in s.chars() {
        let b = winansi(c);
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// Splits `text` into lines that fit into `size_em_per_line` em width (simple word wrapping).
pub fn wrap(face: Face, text: &str, max_em: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if !line.is_empty() && text_width_em(face, &candidate) > max_em {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_and_encoding() {
        assert_eq!(char_width(Face::Bold, '0'), 556);
        assert_eq!(char_width(Face::Regular, 'W'), 944);
        assert!((text_width_em(Face::Bold, "ASN00001") - (0.722 + 0.667 + 0.722 + 5.0 * 0.556)).abs() < 1e-9);
        assert_eq!(char_width(Face::Regular, 'ä'), char_width(Face::Regular, 'a'));
        assert_eq!(pdf_string("a(b)\\ä"), b"(a\\(b\\)\\\\\xe4)".to_vec());
        assert_eq!(winansi('€'), 0x80);
        assert_eq!(winansi('漢'), b'?');
        assert_eq!(wrap(Face::Regular, "aaa bbb ccc", 4.0), vec!["aaa bbb", "ccc"]);
    }
}
