//! QR code generation, a line-by-line port of python-qrcode as used by allauth
//! (`qrcode.make(url, image_factory=SvgPathImage)`), so the SVG is byte-identical.
//!
//! upstream: qrcode 8.2 – qrcode/main.py (QRCode.make/best_fit/makeImpl/map_data/
//! best_mask_pattern/setup_*), qrcode/util.py (optimal_data_chunks, QRData, BitBuffer,
//! create_data, create_bytes, lost_point, mask_func, BCH_*), qrcode/base.py (GF tables,
//! Polynomial, rs_blocks), qrcode/image/svg.py (SvgPathImage, serialised by lxml.etree),
//! qrcode/image/styles/moduledrawers/svg.py (SvgPathSquareDrawer).
//! Defaults: ERROR_CORRECT_M, box_size 10, border 4, optimize 20, fit=True, mask auto.

const MODE_NUMBER: u8 = 1;
const MODE_ALPHA_NUM: u8 = 2;
const MODE_8BIT_BYTE: u8 = 4;

/// `qrcode.constants.ERROR_CORRECT_M` (the integer used in format info and tables).
const ERROR_CORRECT_M: usize = 0;

const ALPHA_NUM: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

const PAD0: u8 = 0xEC;
const PAD1: u8 = 0x11;

const G15: u32 = (1 << 10) | (1 << 8) | (1 << 5) | (1 << 4) | (1 << 2) | (1 << 1) | 1;
const G18: u32 = (1 << 12) | (1 << 11) | (1 << 10) | (1 << 9) | (1 << 8) | (1 << 5) | (1 << 2) | 1;
const G15_MASK: u32 = (1 << 14) | (1 << 12) | (1 << 10) | (1 << 4) | (1 << 1);

const PATTERN_POSITION_TABLE: [&[usize]; 40] = [
    &[],
    &[6, 18],
    &[6, 22],
    &[6, 26],
    &[6, 30],
    &[6, 34],
    &[6, 22, 38],
    &[6, 24, 42],
    &[6, 26, 46],
    &[6, 28, 50],
    &[6, 30, 54],
    &[6, 32, 58],
    &[6, 34, 62],
    &[6, 26, 46, 66],
    &[6, 26, 48, 70],
    &[6, 26, 50, 74],
    &[6, 30, 54, 78],
    &[6, 30, 56, 82],
    &[6, 30, 58, 86],
    &[6, 34, 62, 90],
    &[6, 28, 50, 72, 94],
    &[6, 26, 50, 74, 98],
    &[6, 30, 54, 78, 102],
    &[6, 28, 54, 80, 106],
    &[6, 32, 58, 84, 110],
    &[6, 30, 58, 86, 114],
    &[6, 34, 62, 90, 118],
    &[6, 26, 50, 74, 98, 122],
    &[6, 30, 54, 78, 102, 126],
    &[6, 26, 52, 78, 104, 130],
    &[6, 30, 56, 82, 108, 134],
    &[6, 34, 60, 86, 112, 138],
    &[6, 30, 58, 86, 114, 142],
    &[6, 34, 62, 90, 118, 146],
    &[6, 30, 54, 78, 102, 126, 150],
    &[6, 24, 50, 76, 102, 128, 154],
    &[6, 28, 54, 80, 106, 132, 158],
    &[6, 32, 58, 84, 110, 136, 162],
    &[6, 26, 54, 82, 110, 138, 166],
    &[6, 30, 58, 86, 114, 142, 170],
];

/// `base.RS_BLOCK_TABLE`, only the M rows (offset 1 of each version quadruple).
/// Each entry: (count, total_count, data_count)[, (count, total_count, data_count)].
const RS_BLOCK_TABLE_M: [&[usize]; 40] = [
    &[1, 26, 16],
    &[1, 44, 28],
    &[1, 70, 44],
    &[2, 50, 32],
    &[2, 67, 43],
    &[4, 43, 27],
    &[4, 49, 31],
    &[2, 60, 38, 2, 61, 39],
    &[3, 58, 36, 2, 59, 37],
    &[4, 69, 43, 1, 70, 44],
    &[1, 80, 50, 4, 81, 51],
    &[6, 58, 36, 2, 59, 37],
    &[8, 59, 37, 1, 60, 38],
    &[4, 64, 40, 5, 65, 41],
    &[5, 65, 41, 5, 66, 42],
    &[7, 73, 45, 3, 74, 46],
    &[10, 74, 46, 1, 75, 47],
    &[9, 69, 43, 4, 70, 44],
    &[3, 70, 44, 11, 71, 45],
    &[3, 67, 41, 13, 68, 42],
    &[17, 68, 42],
    &[17, 74, 46],
    &[4, 75, 47, 14, 76, 48],
    &[6, 73, 45, 14, 74, 46],
    &[8, 75, 47, 13, 76, 48],
    &[19, 74, 46, 4, 75, 47],
    &[22, 73, 45, 3, 74, 46],
    &[3, 73, 45, 23, 74, 46],
    &[21, 73, 45, 7, 74, 46],
    &[19, 75, 47, 10, 76, 48],
    &[2, 74, 46, 29, 75, 47],
    &[10, 74, 46, 23, 75, 47],
    &[14, 74, 46, 21, 75, 47],
    &[14, 74, 46, 23, 75, 47],
    &[12, 75, 47, 26, 76, 48],
    &[6, 75, 47, 34, 76, 48],
    &[29, 74, 46, 14, 75, 47],
    &[13, 74, 46, 32, 75, 47],
    &[40, 75, 47, 7, 76, 48],
    &[18, 75, 47, 31, 76, 48],
];

#[derive(Debug, Clone, Copy)]
struct RsBlock {
    total_count: usize,
    data_count: usize,
}

fn rs_blocks(version: usize) -> Vec<RsBlock> {
    let row = RS_BLOCK_TABLE_M[version - 1];
    let mut blocks = Vec::new();
    for chunk in row.chunks(3) {
        for _ in 0..chunk[0] {
            blocks.push(RsBlock { total_count: chunk[1], data_count: chunk[2] });
        }
    }
    blocks
}

/// `BIT_LIMIT_TABLE[ERROR_CORRECT_M]`: index 0 is 0, index v = 8 * sum(data_count).
fn bit_limit_table() -> [usize; 41] {
    let mut t = [0usize; 41];
    for (v, slot) in t.iter_mut().enumerate().skip(1) {
        *slot = 8 * rs_blocks(v).iter().map(|b| b.data_count).sum::<usize>();
    }
    t
}

struct Gf {
    exp: [u8; 256],
    log: [u8; 256],
}

impl Gf {
    fn new() -> Self {
        let mut exp = [0u32; 256];
        for (i, e) in exp.iter_mut().enumerate().take(8) {
            *e = 1 << i;
        }
        for i in 8..256 {
            exp[i] = exp[i - 4] ^ exp[i - 5] ^ exp[i - 6] ^ exp[i - 8];
        }
        let mut log = [0u8; 256];
        for (i, e) in exp.iter().enumerate().take(255) {
            log[*e as usize] = i as u8;
        }
        let mut exp8 = [0u8; 256];
        for (d, s) in exp8.iter_mut().zip(exp.iter()) {
            *d = *s as u8;
        }
        Gf { exp: exp8, log }
    }

    /// `base.glog` (callers never pass 0; python raises there).
    fn glog(&self, n: u8) -> i64 {
        i64::from(self.log[n as usize])
    }

    /// `base.gexp`: `EXP_TABLE[n % 255]` with Python modulo semantics.
    fn gexp(&self, n: i64) -> u8 {
        self.exp[n.rem_euclid(255) as usize]
    }
}

/// `base.Polynomial.__init__`: strip leading zeros (keeping the last element), append `shift` zeros.
fn poly_new(num: &[u8], shift: usize) -> Vec<u8> {
    let mut offset = 0;
    for (i, v) in num.iter().enumerate() {
        offset = i;
        if *v != 0 {
            break;
        }
    }
    let mut out: Vec<u8> = num[offset..].to_vec();
    out.extend(std::iter::repeat_n(0u8, shift));
    out
}

fn poly_mul(gf: &Gf, a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut num = vec![0u8; a.len() + b.len() - 1];
    for (i, item) in a.iter().enumerate() {
        for (j, other) in b.iter().enumerate() {
            num[i + j] ^= gf.gexp(gf.glog(*item) + gf.glog(*other));
        }
    }
    poly_new(&num, 0)
}

fn poly_mod(gf: &Gf, a: Vec<u8>, b: &[u8]) -> Vec<u8> {
    let mut a = a;
    loop {
        if a.len() < b.len() {
            return a;
        }
        let difference = a.len() - b.len();
        let ratio = gf.glog(a[0]) - gf.glog(b[0]);
        let mut num: Vec<u8> = a.iter().zip(b.iter()).map(|(item, other)| item ^ gf.gexp(gf.glog(*other) + ratio)).collect();
        if difference > 0 {
            num.extend_from_slice(&a[a.len() - difference..]);
        }
        let next = poly_new(&num, 0);
        if next.len() >= b.len() && next[0] == 0 {
            // python would raise in glog(0) here; cannot happen for real data
            return next;
        }
        a = next;
    }
}

#[derive(Debug, Clone)]
struct QrData {
    mode: u8,
    data: Vec<u8>,
}

impl QrData {
    fn write(&self, buf: &mut BitBuffer) {
        match self.mode {
            MODE_NUMBER => {
                for chars in self.data.chunks(3) {
                    let bit_length = match chars.len() {
                        3 => 10,
                        2 => 7,
                        _ => 4,
                    };
                    let mut v: u32 = 0;
                    for c in chars {
                        v = v * 10 + u32::from(c - b'0');
                    }
                    buf.put(v, bit_length);
                }
            }
            MODE_ALPHA_NUM => {
                for chars in self.data.chunks(2) {
                    let pos = |c: u8| ALPHA_NUM.iter().position(|x| *x == c).unwrap_or(0) as u32;
                    if chars.len() > 1 {
                        buf.put(pos(chars[0]) * 45 + pos(chars[1]), 11);
                    } else {
                        buf.put(pos(chars[0]), 6);
                    }
                }
            }
            _ => {
                for c in &self.data {
                    buf.put(u32::from(*c), 8);
                }
            }
        }
    }
}

#[derive(Default)]
struct BitBuffer {
    buffer: Vec<u8>,
    length: usize,
}

impl BitBuffer {
    fn put(&mut self, num: u32, length: usize) {
        for i in 0..length {
            let shift = length - i - 1;
            let bit = if shift >= 32 { false } else { ((num >> shift) & 1) == 1 };
            self.put_bit(bit);
        }
    }

    fn put_bit(&mut self, bit: bool) {
        let idx = self.length / 8;
        if self.buffer.len() <= idx {
            self.buffer.push(0);
        }
        if bit {
            self.buffer[idx] |= 0x80 >> (self.length % 8);
        }
        self.length += 1;
    }
}

fn mode_size_class(version: usize) -> u8 {
    if version < 10 {
        0
    } else if version < 27 {
        1
    } else {
        2
    }
}

fn length_in_bits(mode: u8, version: usize) -> usize {
    match (mode_size_class(version), mode) {
        (0, MODE_NUMBER) => 10,
        (0, MODE_ALPHA_NUM) => 9,
        (0, _) => 8,
        (1, MODE_NUMBER) => 12,
        (1, MODE_ALPHA_NUM) => 11,
        (1, _) => 16,
        (_, MODE_NUMBER) => 14,
        (_, MODE_ALPHA_NUM) => 13,
        (_, _) => 16,
    }
}

fn is_alnum_byte(c: u8) -> bool {
    ALPHA_NUM.contains(&c)
}

/// One step of `util._optimal_split`: find the first maximal run of `class` bytes that has at
/// least `min` bytes (equivalent to `re.search(b"[class]{min,}")`, or the whole-string match when
/// `whole` is set, i.e. `^[class]+$`).
fn optimal_split(data: &[u8], class: fn(u8) -> bool, min: usize, whole: bool) -> Vec<(bool, Vec<u8>)> {
    let mut out = Vec::new();
    let mut rest = data;
    while !rest.is_empty() {
        let found = if whole {
            if rest.iter().all(|c| class(*c)) { Some((0, rest.len())) } else { None }
        } else {
            let mut found = None;
            let mut i = 0;
            while i < rest.len() {
                if class(rest[i]) {
                    let start = i;
                    while i < rest.len() && class(rest[i]) {
                        i += 1;
                    }
                    if i - start >= min {
                        found = Some((start, i));
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            found
        };
        let Some((start, end)) = found else { break };
        if start > 0 {
            out.push((false, rest[..start].to_vec()));
        }
        out.push((true, rest[start..end].to_vec()));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        out.push((false, rest.to_vec()));
    }
    out
}

/// `util.optimal_data_chunks(data, minimum)`.
fn optimal_data_chunks(data: &[u8], minimum: usize) -> Vec<QrData> {
    let whole = data.len() <= minimum;
    let mut out = Vec::new();
    for (is_num, chunk) in optimal_split(data, |c| c.is_ascii_digit(), minimum, whole) {
        if is_num {
            out.push(QrData { mode: MODE_NUMBER, data: chunk });
        } else {
            for (is_alpha, sub) in optimal_split(&chunk, is_alnum_byte, minimum, whole) {
                out.push(QrData { mode: if is_alpha { MODE_ALPHA_NUM } else { MODE_8BIT_BYTE }, data: sub });
            }
        }
    }
    out
}

fn bch_digit(mut data: u32) -> i32 {
    let mut digit = 0;
    while data != 0 {
        digit += 1;
        data >>= 1;
    }
    digit
}

fn bch_type_info(data: u32) -> u32 {
    let mut d = data << 10;
    while bch_digit(d) - bch_digit(G15) >= 0 {
        d ^= G15 << (bch_digit(d) - bch_digit(G15));
    }
    ((data << 10) | d) ^ G15_MASK
}

fn bch_type_number(data: u32) -> u32 {
    let mut d = data << 12;
    while bch_digit(d) - bch_digit(G18) >= 0 {
        d ^= G18 << (bch_digit(d) - bch_digit(G18));
    }
    (data << 12) | d
}

fn mask_func(pattern: u8, i: usize, j: usize) -> bool {
    match pattern {
        0 => (i + j).is_multiple_of(2),
        1 => i.is_multiple_of(2),
        2 => j.is_multiple_of(3),
        3 => (i + j).is_multiple_of(3),
        4 => (i / 2 + j / 3).is_multiple_of(2),
        5 => (i * j) % 2 + (i * j) % 3 == 0,
        6 => ((i * j) % 2 + (i * j) % 3).is_multiple_of(2),
        _ => ((i * j) % 3 + (i + j) % 2).is_multiple_of(2),
    }
}

/// Error from QR generation (python: `DataOverflowError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataOverflow;

fn create_bytes(gf: &Gf, buffer: &BitBuffer, blocks: &[RsBlock]) -> Vec<u8> {
    let mut offset = 0;
    let mut max_dc = 0;
    let mut max_ec = 0;
    let mut dcdata: Vec<Vec<u8>> = Vec::new();
    let mut ecdata: Vec<Vec<u8>> = Vec::new();
    for b in blocks {
        let dc_count = b.data_count;
        let ec_count = b.total_count - dc_count;
        max_dc = max_dc.max(dc_count);
        max_ec = max_ec.max(ec_count);
        let current_dc: Vec<u8> = (0..dc_count).map(|i| buffer.buffer.get(i + offset).copied().unwrap_or(0)).collect();
        offset += dc_count;

        let mut rs_poly = vec![1u8];
        for i in 0..ec_count {
            rs_poly = poly_mul(gf, &rs_poly, &[1, gf.gexp(i as i64)]);
        }
        let raw = poly_new(&current_dc, rs_poly.len() - 1);
        let modp = poly_mod(gf, raw, &rs_poly);
        let mod_offset = modp.len() as i64 - ec_count as i64;
        let current_ec: Vec<u8> = (0..ec_count)
            .map(|i| {
                let idx = i as i64 + mod_offset;
                if idx >= 0 { modp[idx as usize] } else { 0 }
            })
            .collect();
        dcdata.push(current_dc);
        ecdata.push(current_ec);
    }
    let mut data = Vec::new();
    for i in 0..max_dc {
        for dc in &dcdata {
            if i < dc.len() {
                data.push(dc[i]);
            }
        }
    }
    for i in 0..max_ec {
        for ec in &ecdata {
            if i < ec.len() {
                data.push(ec[i]);
            }
        }
    }
    data
}

fn create_data(gf: &Gf, version: usize, list: &[QrData]) -> Result<Vec<u8>, DataOverflow> {
    let mut buffer = BitBuffer::default();
    for d in list {
        buffer.put(u32::from(d.mode), 4);
        buffer.put(d.data.len() as u32, length_in_bits(d.mode, version));
        d.write(&mut buffer);
    }
    let blocks = rs_blocks(version);
    let bit_limit: usize = blocks.iter().map(|b| b.data_count * 8).sum();
    if buffer.length > bit_limit {
        return Err(DataOverflow);
    }
    for _ in 0..(bit_limit - buffer.length).min(4) {
        buffer.put_bit(false);
    }
    let delimit = buffer.length % 8;
    if delimit != 0 {
        for _ in 0..(8 - delimit) {
            buffer.put_bit(false);
        }
    }
    let bytes_to_fill = (bit_limit - buffer.length) / 8;
    for i in 0..bytes_to_fill {
        buffer.put(u32::from(if i % 2 == 0 { PAD0 } else { PAD1 }), 8);
    }
    Ok(create_bytes(gf, &buffer, &blocks))
}

type Modules = Vec<Vec<Option<bool>>>;

struct QrCode {
    version: usize,
    count: usize,
    modules: Modules,
    data_cache: Vec<u8>,
}

impl QrCode {
    fn setup_position_probe_pattern(&mut self, row: i64, col: i64) {
        let n = self.count as i64;
        for r in -1..8i64 {
            if row + r <= -1 || n <= row + r {
                continue;
            }
            for c in -1..8i64 {
                if col + c <= -1 || n <= col + c {
                    continue;
                }
                let dark = ((0..=6).contains(&r) && (c == 0 || c == 6))
                    || ((0..=6).contains(&c) && (r == 0 || r == 6))
                    || ((2..=4).contains(&r) && (2..=4).contains(&c));
                self.modules[(row + r) as usize][(col + c) as usize] = Some(dark);
            }
        }
    }

    fn setup_position_adjust_pattern(&mut self) {
        let pos = PATTERN_POSITION_TABLE[self.version - 1];
        for &row in pos {
            for &col in pos {
                if self.modules[row][col].is_some() {
                    continue;
                }
                for r in -2i64..3 {
                    for c in -2i64..3 {
                        let dark = r == -2 || r == 2 || c == -2 || c == 2 || (r == 0 && c == 0);
                        self.modules[(row as i64 + r) as usize][(col as i64 + c) as usize] = Some(dark);
                    }
                }
            }
        }
    }

    fn setup_timing_pattern(&mut self) {
        let n = self.count;
        for r in 8..n.saturating_sub(8) {
            if self.modules[r][6].is_some() {
                continue;
            }
            self.modules[r][6] = Some(r % 2 == 0);
        }
        for c in 8..n.saturating_sub(8) {
            if self.modules[6][c].is_some() {
                continue;
            }
            self.modules[6][c] = Some(c % 2 == 0);
        }
    }

    fn setup_type_number(&mut self, test: bool) {
        let bits = bch_type_number(self.version as u32);
        let n = self.count;
        for i in 0..18 {
            let m = !test && ((bits >> i) & 1) == 1;
            self.modules[i / 3][i % 3 + n - 8 - 3] = Some(m);
        }
        for i in 0..18 {
            let m = !test && ((bits >> i) & 1) == 1;
            self.modules[i % 3 + n - 8 - 3][i / 3] = Some(m);
        }
    }

    fn setup_type_info(&mut self, test: bool, mask: u8) {
        let data = ((ERROR_CORRECT_M as u32) << 3) | u32::from(mask);
        let bits = bch_type_info(data);
        let n = self.count;
        for i in 0..15 {
            let m = !test && ((bits >> i) & 1) == 1;
            if i < 6 {
                self.modules[i][8] = Some(m);
            } else if i < 8 {
                self.modules[i + 1][8] = Some(m);
            } else {
                self.modules[n - 15 + i][8] = Some(m);
            }
        }
        for i in 0..15 {
            let m = !test && ((bits >> i) & 1) == 1;
            if i < 8 {
                self.modules[8][n - i - 1] = Some(m);
            } else if i < 9 {
                self.modules[8][15 - i - 1 + 1] = Some(m);
            } else {
                self.modules[8][15 - i - 1] = Some(m);
            }
        }
        self.modules[n - 8][8] = Some(!test);
    }

    fn map_data(&mut self, mask: u8) {
        let n = self.count as i64;
        let mut inc: i64 = -1;
        let mut row: i64 = n - 1;
        let mut bit_index: i32 = 7;
        let mut byte_index = 0usize;
        let data_len = self.data_cache.len();
        let mut col = n - 1;
        while col > 0 {
            let mut c0 = col;
            if c0 <= 6 {
                c0 -= 1;
            }
            let col_range = [c0, c0 - 1];
            loop {
                for &c in &col_range {
                    let (ru, cu) = (row as usize, c as usize);
                    if self.modules[ru][cu].is_none() {
                        let mut dark = false;
                        if byte_index < data_len {
                            dark = ((self.data_cache[byte_index] >> bit_index) & 1) == 1;
                        }
                        if mask_func(mask, ru, cu) {
                            dark = !dark;
                        }
                        self.modules[ru][cu] = Some(dark);
                        bit_index -= 1;
                        if bit_index == -1 {
                            byte_index += 1;
                            bit_index = 7;
                        }
                    }
                }
                row += inc;
                if row < 0 || n <= row {
                    row -= inc;
                    inc = -inc;
                    break;
                }
            }
            col -= 2;
        }
    }

    fn make_impl(&mut self, blank: &Modules, test: bool, mask: u8) {
        self.modules = blank.clone();
        self.setup_type_info(test, mask);
        if self.version >= 7 {
            self.setup_type_number(test);
        }
        self.map_data(mask);
    }
}

fn lost_point(m: &[Vec<bool>]) -> i64 {
    let n = m.len();
    lost_point_level1(m, n) + lost_point_level2(m, n) + lost_point_level3(m, n) + lost_point_level4(m, n)
}

fn lost_point_level1(m: &[Vec<bool>], n: usize) -> i64 {
    let mut container = vec![0i64; n + 1];
    for row in m.iter() {
        let mut prev = row[0];
        let mut length = 0usize;
        for &v in row.iter() {
            if v == prev {
                length += 1;
            } else {
                if length >= 5 {
                    container[length] += 1;
                }
                length = 1;
                prev = v;
            }
        }
        if length >= 5 {
            container[length] += 1;
        }
    }
    for col in 0..n {
        let mut prev = m[0][col];
        let mut length = 0usize;
        for row in m.iter() {
            if row[col] == prev {
                length += 1;
            } else {
                if length >= 5 {
                    container[length] += 1;
                }
                length = 1;
                prev = row[col];
            }
        }
        if length >= 5 {
            container[length] += 1;
        }
    }
    (5..=n).map(|l| container[l] * (l as i64 - 2)).sum()
}

fn lost_point_level2(m: &[Vec<bool>], n: usize) -> i64 {
    let mut lost = 0;
    for row in 0..n.saturating_sub(1) {
        let this_row = &m[row];
        let next_row = &m[row + 1];
        let mut it = 0..n - 1;
        while let Some(col) = it.next() {
            let top_right = this_row[col + 1];
            if top_right != next_row[col + 1] {
                it.next();
            } else if top_right != this_row[col] || top_right != next_row[col] {
                continue;
            } else {
                lost += 3;
            }
        }
    }
    lost
}

fn finder_like(get: &dyn Fn(usize) -> bool) -> bool {
    !get(1)
        && get(4)
        && !get(5)
        && get(6)
        && !get(9)
        && ((get(0) && get(2) && get(3) && !get(7) && !get(8) && !get(10))
            || (!get(0) && !get(2) && !get(3) && get(7) && get(8) && get(10)))
}

#[allow(clippy::needless_range_loop)] // faithful port of util._lost_point_level3
fn lost_point_level3(m: &[Vec<bool>], n: usize) -> i64 {
    let mut lost = 0;
    let short = n.saturating_sub(10);
    for this_row in m.iter() {
        let mut it = 0..short;
        while let Some(col) = it.next() {
            if finder_like(&|k| this_row[col + k]) {
                lost += 40;
            }
            if this_row[col + 10] {
                it.next();
            }
        }
    }
    for col in 0..n {
        let mut it = 0..short;
        while let Some(row) = it.next() {
            if finder_like(&|k| m[row + k][col]) {
                lost += 40;
            }
            if m[row + 10][col] {
                it.next();
            }
        }
    }
    lost
}

fn lost_point_level4(m: &[Vec<bool>], n: usize) -> i64 {
    let dark: usize = m.iter().map(|r| r.iter().filter(|v| **v).count()).sum();
    let percent = dark as f64 / (n * n) as f64;
    let rating = ((percent * 100.0 - 50.0).abs() / 5.0) as i64;
    rating * 10
}

fn to_bool(m: &Modules) -> Vec<Vec<bool>> {
    m.iter().map(|r| r.iter().map(|v| v.unwrap_or(false)).collect()).collect()
}

/// `QRCode().add_data(data); make(fit=True)` → module matrix (without border).
pub fn make_matrix(data: &[u8]) -> Result<Vec<Vec<bool>>, DataOverflow> {
    let gf = Gf::new();
    let list = optimal_data_chunks(data, 20);
    let table = bit_limit_table();

    // best_fit(start=1), recursing while the mode-size class changes
    let mut start = 1usize;
    let version = loop {
        let mut buffer = BitBuffer::default();
        for d in &list {
            buffer.put(u32::from(d.mode), 4);
            buffer.put(d.data.len() as u32, length_in_bits(d.mode, start));
            d.write(&mut buffer);
        }
        let needed = buffer.length;
        // bisect_left(table, needed, lo=start)
        let mut v = start;
        while v < table.len() && table[v] < needed {
            v += 1;
        }
        if v == 41 {
            return Err(DataOverflow);
        }
        if mode_size_class(v) != mode_size_class(start) {
            start = v;
            continue;
        }
        break v;
    };

    let count = version * 4 + 17;
    let mut qr = QrCode { version, count, modules: vec![vec![None; count]; count], data_cache: Vec::new() };
    qr.setup_position_probe_pattern(0, 0);
    qr.setup_position_probe_pattern(count as i64 - 7, 0);
    qr.setup_position_probe_pattern(0, count as i64 - 7);
    qr.setup_position_adjust_pattern();
    qr.setup_timing_pattern();
    let blank = qr.modules.clone();
    qr.data_cache = create_data(&gf, version, &list)?;

    let mut pattern = 0u8;
    let mut min_lost = 0i64;
    for i in 0..8u8 {
        qr.make_impl(&blank, true, i);
        let lp = lost_point(&to_bool(&qr.modules));
        if i == 0 || min_lost > lp {
            min_lost = lp;
            pattern = i;
        }
    }
    qr.make_impl(&blank, false, pattern);
    Ok(to_bool(&qr.modules))
}

/// `allauth DefaultMFAAdapter.build_totp_svg(url)`: SvgPathImage written by lxml.
pub fn build_svg(data: &str) -> Result<String, DataOverflow> {
    let modules = make_matrix(data.as_bytes())?;
    let border = 4usize;
    let n = modules.len();
    let dim = n + border * 2; // pixel_size / 10 with box_size 10
    let mut d = String::with_capacity(n * n * 10);
    for (r, row) in modules.iter().enumerate() {
        for (c, dark) in row.iter().enumerate() {
            if *dark {
                let x0 = c + border;
                let y0 = r + border;
                d.push_str(&format!("M{x0},{y0}H{}V{}H{x0}z", x0 + 1, y0 + 1));
            }
        }
    }
    Ok(format!(
        "<?xml version='1.0' encoding='UTF-8'?>\n<svg width=\"{dim}mm\" height=\"{dim}mm\" version=\"1.1\" viewBox=\"0 0 {dim} {dim}\" xmlns=\"http://www.w3.org/2000/svg\"><path d=\"{d}\" id=\"qr-path\" fill=\"#000000\" fill-opacity=\"1\" fill-rule=\"nonzero\" stroke=\"none\"/></svg>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Generated with python-qrcode 8.2 + lxml 6.1.0 (tests/parity/pl-auth-api/qr_vectors.py).
    const VECTORS: &str = include_str!("../tests/qr_vectors.txt");

    #[test]
    fn matches_python_qrcode() {
        let mut n = 0;
        let mut lines = VECTORS.lines();
        while let Some(input) = lines.next() {
            let expected = lines.next().expect("vector svg line");
            let expected = expected.replace("\\n", "\n");
            let got = build_svg(input).expect("fits");
            assert_eq!(got, expected, "input {input:?}");
            n += 1;
        }
        assert!(n >= 10, "too few vectors: {n}");
    }

    #[test]
    fn rs_table_matches_known_capacities() {
        let t = bit_limit_table();
        // ISO 18004 data capacity (codewords) for level M: v1=16, v10=216, v40=2334
        assert_eq!(t[1], 16 * 8);
        assert_eq!(t[10], 216 * 8);
        assert_eq!(t[40], 2334 * 8);
    }
}
