//! serde helper: `Vec<f32>` as base64 of little-endian bytes (compact inside the JSON model file).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(v: &[f32], s: S) -> Result<S::Ok, S::Error> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for x in v {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    s.serialize_str(&STANDARD.encode(bytes))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
    let s = String::deserialize(d)?;
    let bytes = STANDARD.decode(s.as_bytes()).map_err(serde::de::Error::custom)?;
    if bytes.len() % 4 != 0 {
        return Err(serde::de::Error::custom("f32 array length"));
    }
    Ok(bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}
