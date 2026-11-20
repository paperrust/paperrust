//! Order-independent table checksum over canonical values.
//!
//! Row hash = SHA-256 over all cells (NULL = 0x00, value = 0x01 + length (u64 LE) + UTF-8 bytes).
//! Table hash = SHA-256(XOR of all row hashes ‖ sum (mod 2^128) of the first 16 bytes ‖ count).
//! Order-independent because SQLite/MySQL/PostgreSQL sort string primary keys
//! (e.g. `django_session.session_key`) differently.

use sha2::{Digest, Sha256};

pub type RowHash = [u8; 32];

pub fn row_hash<'a>(cells: impl IntoIterator<Item = Option<&'a str>>) -> RowHash {
    let mut h = Sha256::new();
    for c in cells {
        match c {
            None => h.update([0u8]),
            Some(s) => {
                h.update([1u8]);
                h.update((s.len() as u64).to_le_bytes());
                h.update(s.as_bytes());
            }
        }
    }
    h.finalize().into()
}

#[derive(Debug, Default, Clone)]
pub struct TableHash {
    xor: [u8; 32],
    sum: u128,
    pub rows: u64,
}

impl TableHash {
    pub fn add(&mut self, r: &RowHash) {
        for (a, b) in self.xor.iter_mut().zip(r.iter()) {
            *a ^= b;
        }
        let mut first = [0u8; 16];
        first.copy_from_slice(&r[..16]);
        self.sum = self.sum.wrapping_add(u128::from_le_bytes(first));
        self.rows += 1;
    }

    pub fn hex(&self) -> String {
        let mut h = Sha256::new();
        h.update(self.xor);
        h.update(self.sum.to_le_bytes());
        h.update(self.rows.to_le_bytes());
        crate::value::hex(&h.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_independent_and_null_distinct() {
        let a = row_hash([Some("1"), None]);
        let b = row_hash([Some("2"), Some("")]);
        let mut x = TableHash::default();
        x.add(&a);
        x.add(&b);
        let mut y = TableHash::default();
        y.add(&b);
        y.add(&a);
        assert_eq!(x.hex(), y.hex());
        assert_ne!(row_hash([None]), row_hash([Some("")]));
        assert_ne!(row_hash([Some("ab"), Some("c")]), row_hash([Some("a"), Some("bc")]));
    }
}
