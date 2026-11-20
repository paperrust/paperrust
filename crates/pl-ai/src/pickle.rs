//! Pickle (protocol 5) for the shared Django cache: `documents.caching.SuggestionCacheData`
//! (frozen dataclass: `classifier_version`, `classifier_hash`, `suggestions`) – byte-exact like
//! CPython 3.12 `pickle.dumps(obj, 5)` (Django `RedisSerializer`), golden `golden_w8.json` → `pickle`.
//!
//! Reading accepts only the opcodes that CPython produces for exactly this structure; the only
//! allowed class is `documents.caching.SuggestionCacheData` (no arbitrary `find_class`).

use serde_json::{Map, Value};

const FRAME_TARGET: usize = 64 * 1024;

struct Writer {
    out: Vec<u8>,
    cur: Vec<u8>,
    memo: usize,
}

impl Writer {
    fn commit(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        if self.cur.len() >= 4 {
            self.out.push(0x95);
            self.out.extend_from_slice(&(self.cur.len() as u64).to_le_bytes());
        }
        self.out.append(&mut self.cur);
    }
    fn begin_obj(&mut self) {
        if self.cur.len() >= FRAME_TARGET {
            self.commit();
        }
    }
    fn memoize(&mut self) -> usize {
        self.cur.push(0x94);
        self.memo += 1;
        self.memo - 1
    }
    fn get(&mut self, idx: usize) {
        if idx < 256 {
            self.cur.extend_from_slice(&[b'h', idx as u8]);
        } else {
            self.cur.push(b'j');
            self.cur.extend_from_slice(&(idx as u32).to_le_bytes());
        }
    }
    fn str(&mut self, s: &str) -> usize {
        self.begin_obj();
        let b = s.as_bytes();
        if b.len() < 256 {
            self.cur.push(0x8c);
            self.cur.push(b.len() as u8);
            self.cur.extend_from_slice(b);
        } else {
            let mut hdr = vec![b'X'];
            hdr.extend_from_slice(&(b.len() as u32).to_le_bytes());
            if b.len() >= FRAME_TARGET {
                // framer.write_large_bytes: close the frame, payload unframed
                self.cur.extend_from_slice(&hdr);
                self.commit();
                self.out.extend_from_slice(b);
            } else {
                self.cur.extend_from_slice(&hdr);
                self.cur.extend_from_slice(b);
            }
        }
        self.memoize()
    }
    fn int(&mut self, v: i64) {
        self.begin_obj();
        if (0..256).contains(&v) {
            self.cur.extend_from_slice(&[b'K', v as u8]);
        } else if (256..65536).contains(&v) {
            self.cur.push(b'M');
            self.cur.extend_from_slice(&(v as u16).to_le_bytes());
        } else if (i32::MIN as i64..=i32::MAX as i64).contains(&v) {
            self.cur.push(b'J');
            self.cur.extend_from_slice(&(v as i32).to_le_bytes());
        } else {
            // LONG1: smallest two's-complement representation, little endian
            let mut bytes = v.to_le_bytes().to_vec();
            while bytes.len() > 1 {
                let last = bytes[bytes.len() - 1];
                let prev = bytes[bytes.len() - 2];
                if (last == 0 && prev & 0x80 == 0) || (last == 0xff && prev & 0x80 != 0) {
                    bytes.pop();
                } else {
                    break;
                }
            }
            self.cur.extend_from_slice(&[0x8a, bytes.len() as u8]);
            self.cur.extend_from_slice(&bytes);
        }
    }
    fn value(&mut self, v: &Value, key_memo: &mut Vec<(String, usize)>) {
        match v {
            Value::Null => {
                self.begin_obj();
                self.cur.push(b'N')
            }
            Value::Bool(b) => {
                self.begin_obj();
                self.cur.push(if *b { 0x88 } else { 0x89 })
            }
            Value::Number(n) => match n.as_i64() {
                Some(i) => self.int(i),
                None => {
                    self.begin_obj();
                    self.cur.push(b'G');
                    self.cur.extend_from_slice(&n.as_f64().unwrap_or(0.0).to_be_bytes());
                }
            },
            Value::String(s) => {
                self.str(s);
            }
            Value::Array(a) => {
                self.begin_obj();
                self.cur.push(b']');
                self.memoize();
                for batch in a.chunks(1000) {
                    if batch.len() == 1 {
                        self.value(&batch[0], key_memo);
                        self.cur.push(b'a');
                    } else {
                        self.cur.push(b'(');
                        for x in batch {
                            self.value(x, key_memo);
                        }
                        self.cur.push(b'e');
                    }
                }
            }
            Value::Object(m) => self.dict(m, key_memo),
        }
    }
    /// Dict; keys are (as in Python) interned identifiers → BINGET on the second occurrence.
    fn dict(&mut self, m: &Map<String, Value>, key_memo: &mut Vec<(String, usize)>) {
        self.begin_obj();
        self.cur.push(b'}');
        self.memoize();
        let items: Vec<(&String, &Value)> = m.iter().collect();
        for batch in items.chunks(1000) {
            let many = batch.len() > 1;
            if many {
                self.cur.push(b'(');
            }
            for (k, v) in batch {
                if let Some((_, idx)) = key_memo.iter().find(|(s, _)| s == *k) {
                    let idx = *idx;
                    self.begin_obj();
                    self.get(idx);
                } else {
                    let idx = self.str(k);
                    key_memo.push(((*k).clone(), idx));
                }
                self.value(v, key_memo);
            }
            self.cur.push(if many { b'u' } else { b's' });
        }
    }
}

/// `pickle.dumps(SuggestionCacheData(version, backend, suggestions), 5)`.
pub fn dumps_suggestion_cache(version: i64, backend: &str, suggestions: &Value) -> Vec<u8> {
    let mut w = Writer { out: vec![0x80, 5], cur: Vec::new(), memo: 0 };
    w.str("documents.caching");
    w.str("SuggestionCacheData");
    w.cur.push(0x93); // STACK_GLOBAL
    w.memoize();
    w.cur.push(b')');
    w.cur.push(0x81); // NEWOBJ
    w.memoize();
    let mut state = Map::new();
    state.insert("classifier_version".into(), Value::from(version));
    state.insert("classifier_hash".into(), Value::from(backend));
    state.insert("suggestions".into(), suggestions.clone());
    let mut km = Vec::new();
    w.dict(&state, &mut km);
    w.cur.push(b'b'); // BUILD
    w.cur.push(b'.');
    w.commit();
    w.out
}

/// `pickle.dumps(str, 5)` (generation token).
pub fn dumps_str(s: &str) -> Vec<u8> {
    let mut w = Writer { out: vec![0x80, 5], cur: Vec::new(), memo: 0 };
    w.str(s);
    w.cur.push(b'.');
    w.commit();
    w.out
}

#[derive(Debug, Clone, PartialEq)]
enum Obj {
    V(Value),
    Mark,
    Class,
    Inst(Option<Value>),
    Tuple(Vec<Value>),
}

/// Cache entry that was read.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    Suggestion { version: i64, hash: String, suggestions: Value },
    Str(String),
    Other,
}

/// Reads a pickle (only the forms above). Error → `Err`.
pub fn loads(data: &[u8]) -> Result<Loaded, String> {
    let mut st: Vec<Obj> = Vec::new();
    let mut memo: Vec<Obj> = Vec::new();
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Result<&[u8], String> {
        let s = data.get(*i..*i + n).ok_or("truncated pickle")?;
        *i += n;
        Ok(s)
    };
    let u32le = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    fn pop_mark(st: &mut Vec<Obj>) -> Result<Vec<Obj>, String> {
        let pos = st.iter().rposition(|o| *o == Obj::Mark).ok_or("no mark")?;
        let items = st.split_off(pos + 1);
        st.pop();
        Ok(items)
    }
    fn as_val(o: Obj) -> Result<Value, String> {
        match o {
            Obj::V(v) => Ok(v),
            Obj::Tuple(t) => Ok(Value::Array(t)),
            _ => Err("unexpected object".into()),
        }
    }
    loop {
        let op = *data.get(i).ok_or("truncated pickle")?;
        i += 1;
        match op {
            0x80 => {
                take(&mut i, 1)?;
            }
            0x95 => {
                take(&mut i, 8)?;
            }
            0x94 => memo.push(st.last().cloned().ok_or("empty stack")?),
            b'q' => {
                let n = take(&mut i, 1)?[0] as usize;
                let v = st.last().cloned().ok_or("empty stack")?;
                if memo.len() <= n {
                    memo.resize(n + 1, Obj::Mark);
                }
                memo[n] = v;
            }
            b'h' => {
                let n = take(&mut i, 1)?[0] as usize;
                st.push(memo.get(n).cloned().ok_or("bad memo")?);
            }
            b'j' => {
                let n = u32le(take(&mut i, 4)?);
                st.push(memo.get(n).cloned().ok_or("bad memo")?);
            }
            0x8c => {
                let n = take(&mut i, 1)?[0] as usize;
                let s = std::str::from_utf8(take(&mut i, n)?).map_err(|e| e.to_string())?;
                st.push(Obj::V(Value::String(s.to_string())));
            }
            b'X' => {
                let n = u32le(take(&mut i, 4)?);
                let s = std::str::from_utf8(take(&mut i, n)?).map_err(|e| e.to_string())?;
                st.push(Obj::V(Value::String(s.to_string())));
            }
            b'K' => st.push(Obj::V(Value::from(take(&mut i, 1)?[0]))),
            b'M' => {
                let b = take(&mut i, 2)?;
                st.push(Obj::V(Value::from(u16::from_le_bytes([b[0], b[1]]))));
            }
            b'J' => {
                let b = take(&mut i, 4)?;
                st.push(Obj::V(Value::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))));
            }
            0x8a => {
                let n = take(&mut i, 1)?[0] as usize;
                let b = take(&mut i, n)?;
                if n > 8 {
                    return Err("long too large".into());
                }
                let mut a = if b.last().is_some_and(|x| x & 0x80 != 0) { [0xffu8; 8] } else { [0u8; 8] };
                a[..n].copy_from_slice(b);
                st.push(Obj::V(Value::from(i64::from_le_bytes(a))));
            }
            b'G' => {
                let b = take(&mut i, 8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                st.push(Obj::V(serde_json::Number::from_f64(f64::from_be_bytes(a)).map(Value::Number).unwrap_or(Value::Null)));
            }
            b'N' => st.push(Obj::V(Value::Null)),
            0x88 => st.push(Obj::V(Value::Bool(true))),
            0x89 => st.push(Obj::V(Value::Bool(false))),
            b']' => st.push(Obj::V(Value::Array(Vec::new()))),
            b'}' => st.push(Obj::V(Value::Object(Map::new()))),
            b')' => st.push(Obj::Tuple(Vec::new())),
            b'(' => st.push(Obj::Mark),
            b'a' => {
                let v = as_val(st.pop().ok_or("empty stack")?)?;
                match st.last_mut() {
                    Some(Obj::V(Value::Array(a))) => a.push(v),
                    _ => return Err("append to non-list".into()),
                }
                sync_memo(&st, &mut memo);
            }
            b'e' => {
                let items = pop_mark(&mut st)?;
                let vals: Result<Vec<Value>, String> = items.into_iter().map(as_val).collect();
                match st.last_mut() {
                    Some(Obj::V(Value::Array(a))) => a.extend(vals?),
                    _ => return Err("appends to non-list".into()),
                }
                sync_memo(&st, &mut memo);
            }
            b's' => {
                let v = as_val(st.pop().ok_or("empty stack")?)?;
                let k = as_val(st.pop().ok_or("empty stack")?)?;
                let Value::String(k) = k else { return Err("non-str key".into()) };
                match st.last_mut() {
                    Some(Obj::V(Value::Object(m))) => {
                        m.insert(k, v);
                    }
                    _ => return Err("setitem to non-dict".into()),
                }
                sync_memo(&st, &mut memo);
            }
            b'u' => {
                let items = pop_mark(&mut st)?;
                let mut it = items.into_iter();
                let mut pairs = Vec::new();
                while let (Some(k), Some(v)) = (it.next(), it.next()) {
                    let Value::String(k) = as_val(k)? else { return Err("non-str key".into()) };
                    pairs.push((k, as_val(v)?));
                }
                match st.last_mut() {
                    Some(Obj::V(Value::Object(m))) => {
                        for (k, v) in pairs {
                            m.insert(k, v);
                        }
                    }
                    _ => return Err("setitems to non-dict".into()),
                }
                sync_memo(&st, &mut memo);
            }
            0x93 => {
                let name = as_val(st.pop().ok_or("empty stack")?)?;
                let module = as_val(st.pop().ok_or("empty stack")?)?;
                if module.as_str() != Some("documents.caching") || name.as_str() != Some("SuggestionCacheData") {
                    return Err("forbidden global".into());
                }
                st.push(Obj::Class);
            }
            0x81 => {
                let _args = st.pop();
                match st.pop() {
                    Some(Obj::Class) => st.push(Obj::Inst(None)),
                    _ => return Err("NEWOBJ without class".into()),
                }
            }
            b'b' => {
                let state = as_val(st.pop().ok_or("empty stack")?)?;
                match st.last_mut() {
                    Some(Obj::Inst(s)) => *s = Some(state),
                    _ => return Err("BUILD without instance".into()),
                }
            }
            b'.' => break,
            other => return Err(format!("unsupported pickle opcode 0x{other:02x}")),
        }
    }
    match st.pop() {
        Some(Obj::Inst(Some(Value::Object(m)))) => Ok(Loaded::Suggestion {
            version: m.get("classifier_version").and_then(Value::as_i64).unwrap_or(-1),
            hash: m.get("classifier_hash").and_then(Value::as_str).unwrap_or("").to_string(),
            suggestions: m.get("suggestions").cloned().unwrap_or(Value::Null),
        }),
        Some(Obj::V(Value::String(s))) => Ok(Loaded::Str(s)),
        _ => Ok(Loaded::Other),
    }
}

/// Keep the memo copy of a modified container current (Python shares references; here
/// values are copied – for the structures that occur, containers are not referenced
/// again after being filled, except strings).
fn sync_memo(st: &[Obj], memo: &mut [Obj]) {
    let _ = (st, memo);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_pickle() {
        assert_eq!(
            dumps_str("0123456789abcdef0123456789abcdef"),
            b"\x80\x05\x95$\x00\x00\x00\x00\x00\x00\x00\x8c 0123456789abcdef0123456789abcdef\x94.".to_vec()
        );
        assert_eq!(loads(&dumps_str("abc")).unwrap(), Loaded::Str("abc".into()));
        assert!(loads(b"\x80\x05\x8c\x02os\x94\x8c\x06system\x94\x93.").is_err());
    }
}
