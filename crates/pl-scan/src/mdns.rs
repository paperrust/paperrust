//! Scanner discovery via mDNS/DNS-SD (RFC 6762/6763), own minimal implementation without a new dependency.
//!
//! A query for `_uscan._tcp.local` and `_uscans._tcp.local` (PTR) goes to
//! `224.0.0.251:5353`, **from an arbitrary port**. According to RFC 6762 §6.7 this is a
//! "legacy unicast" query: responders send the answer straight back to this port. So
//! PaperRust does not need port 5353 (no conflict with Avahi/Bonjour/Windows mDNS).
//!
//! From PTR → SRV (host, port) → A (IPv4) and TXT (`rs` = path, `ty` = model, `UUID`) the
//! eSCL address `http(s)://ip:port/<rs>/` is built.
//!
//! Limits (contract "Known differences"): IPv4 only, only the system's default interface;
//! in Docker with a bridge network no answers arrive → enter the scanner by IP address.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Serialize;

pub const SERVICE_TYPES: [&str; 2] = ["_uscan._tcp.local", "_uscans._tcp.local"];
const MDNS_ADDR: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;

const T_A: u16 = 1;
const T_PTR: u16 = 12;
const T_TXT: u16 = 16;
const T_SRV: u16 = 33;

/// Found scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Found {
    /// Instance name (`HP OfficeJet Pro 9010 [ABC123]`)
    pub name: String,
    pub base_url: String,
    pub model: Option<String>,
    pub uuid: Option<String>,
    pub tls: bool,
}

/// Query packet (ID 0, two PTR questions, QU bit set).
pub fn build_query() -> Vec<u8> {
    let mut p = vec![0, 0, 0, 0, 0, SERVICE_TYPES.len() as u8, 0, 0, 0, 0, 0, 0];
    for t in SERVICE_TYPES {
        for label in t.split('.') {
            p.push(label.len() as u8);
            p.extend_from_slice(label.as_bytes());
        }
        p.push(0);
        p.extend_from_slice(&T_PTR.to_be_bytes());
        p.extend_from_slice(&0x8001u16.to_be_bytes()); // IN + unicast-response
    }
    p
}

/// Read a domain name from `pos` (with compression). → (name, position after the name).
fn read_name(msg: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end: Option<usize> = None;
    let mut jumps = 0;
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 == 0xC0 {
            let off = ((len & 0x3F) << 8) | *msg.get(pos + 1)? as usize;
            if end.is_none() {
                end = Some(pos + 2);
            }
            jumps += 1;
            if jumps > 32 || off >= msg.len() {
                return None;
            }
            pos = off;
            continue;
        }
        if len > 63 {
            return None;
        }
        let label = msg.get(pos + 1..pos + 1 + len)?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        pos += 1 + len;
        if labels.len() > 128 {
            return None;
        }
    }
    Some((labels.join("."), end.unwrap_or(pos)))
}

#[derive(Debug, Default)]
pub struct Records {
    pub ptr: Vec<(String, String)>,
    pub srv: HashMap<String, (String, u16)>,
    pub txt: HashMap<String, HashMap<String, String>>,
    pub a: HashMap<String, Ipv4Addr>,
}

fn lower(s: &str) -> String {
    s.trim_end_matches('.').to_ascii_lowercase()
}

/// Read a response packet (answers, authority, additional) and collect into `rec`.
pub fn parse_response(msg: &[u8], rec: &mut Records) -> Option<()> {
    if msg.len() < 12 {
        return None;
    }
    let qd = u16::from_be_bytes([msg[4], msg[5]]) as usize;
    let rr = u16::from_be_bytes([msg[6], msg[7]]) as usize + u16::from_be_bytes([msg[8], msg[9]]) as usize + u16::from_be_bytes([msg[10], msg[11]]) as usize;
    let mut pos = 12;
    for _ in 0..qd {
        let (_, p) = read_name(msg, pos)?;
        pos = p + 4;
    }
    for _ in 0..rr.min(512) {
        let (name, p) = read_name(msg, pos)?;
        let hdr = msg.get(p..p + 10)?;
        let ty = u16::from_be_bytes([hdr[0], hdr[1]]);
        let rdlen = u16::from_be_bytes([hdr[8], hdr[9]]) as usize;
        let rdata_start = p + 10;
        let rdata = msg.get(rdata_start..rdata_start + rdlen)?;
        match ty {
            T_PTR => {
                if let Some((target, _)) = read_name(msg, rdata_start) {
                    rec.ptr.push((lower(&name), target.trim_end_matches('.').to_string()));
                }
            }
            T_SRV if rdlen >= 6 => {
                let port = u16::from_be_bytes([rdata[4], rdata[5]]);
                if let Some((target, _)) = read_name(msg, rdata_start + 6) {
                    rec.srv.insert(lower(&name), (lower(&target), port));
                }
            }
            T_TXT => {
                let mut kv = HashMap::new();
                let mut i = 0;
                while i < rdata.len() {
                    let l = rdata[i] as usize;
                    let Some(s) = rdata.get(i + 1..i + 1 + l) else { break };
                    let s = String::from_utf8_lossy(s);
                    let (k, v) = s.split_once('=').unwrap_or((&s, ""));
                    kv.insert(k.to_ascii_lowercase(), v.to_string());
                    i += 1 + l;
                }
                rec.txt.insert(lower(&name), kv);
            }
            T_A if rdlen == 4 => {
                rec.a.insert(lower(&name), Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3]));
            }
            _ => {}
        }
        pos = rdata_start + rdlen;
    }
    Some(())
}

/// Collected records → scanners. `fallback` = sender address per instance (without an A record).
pub fn assemble(rec: &Records, fallback: &HashMap<String, IpAddr>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for (service, instance) in &rec.ptr {
        let tls = service.starts_with("_uscans.");
        if !SERVICE_TYPES.iter().any(|t| lower(t) == *service) {
            continue;
        }
        let key = lower(instance);
        let Some((target, port)) = rec.srv.get(&key) else { continue };
        let ip: Option<IpAddr> = rec.a.get(target).map(|a| IpAddr::V4(*a)).or_else(|| fallback.get(&key).copied());
        let Some(ip) = ip else { continue };
        let txt = rec.txt.get(&key);
        let rs = txt.and_then(|t| t.get("rs")).map(|s| s.trim_matches('/').to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "eSCL".into());
        let host = match ip {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        };
        let scheme = if tls { "https" } else { "http" };
        let base_url = format!("{scheme}://{host}:{port}/{rs}/");
        let display = instance.strip_suffix(&format!(".{}", service.trim_end_matches('.'))).unwrap_or(instance).to_string();
        let f = Found {
            name: display.replace("\\032", " "),
            base_url,
            model: txt.and_then(|t| t.get("ty")).cloned().filter(|s| !s.is_empty()),
            uuid: txt.and_then(|t| t.get("uuid")).cloned().filter(|s| !s.is_empty()),
            tls,
        };
        // The same device via http and https: do not prefer http – show both, https first
        if !out.iter().any(|o| o.base_url == f.base_url) {
            out.push(f);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(b.tls.cmp(&a.tls)));
    out
}

/// Search the local network (collect answers for `timeout`).
pub async fn discover(timeout: Duration) -> std::io::Result<Vec<Found>> {
    let sock = tokio::net::UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await?;
    let _ = sock.set_multicast_ttl_v4(255);
    let q = build_query();
    sock.send_to(&q, SocketAddr::from((MDNS_ADDR, MDNS_PORT))).await?;
    let mut rec = Records::default();
    let mut senders: Vec<(IpAddr, Vec<String>)> = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf = vec![0u8; 9000];
    let mut resent = false;
    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        // Repeat once after half the time (lost packets)
        let half = deadline - timeout / 2;
        let until = if !resent && now < half { half } else { deadline };
        match tokio::time::timeout_at(until, sock.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                let mut one = Records::default();
                if parse_response(&buf[..n], &mut one).is_some() {
                    senders.push((from.ip(), one.srv.keys().cloned().collect()));
                    rec.ptr.extend(one.ptr);
                    rec.srv.extend(one.srv);
                    rec.txt.extend(one.txt);
                    rec.a.extend(one.a);
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                if !resent {
                    resent = true;
                    let _ = sock.send_to(&q, SocketAddr::from((MDNS_ADDR, MDNS_PORT))).await;
                }
            }
        }
    }
    let mut fallback = HashMap::new();
    for (ip, instances) in senders {
        for i in instances {
            fallback.entry(i).or_insert(ip);
        }
    }
    Ok(assemble(&rec, &fallback))
}

#[cfg(test)]
pub(crate) mod testpkt {
    //! Build response packets (for tests).
    fn name(p: &mut Vec<u8>, n: &str) {
        for l in n.split('.') {
            p.push(l.len() as u8);
            p.extend_from_slice(l.as_bytes());
        }
        p.push(0);
    }
    fn rr(p: &mut Vec<u8>, n: &str, ty: u16, rdata: &[u8]) {
        name(p, n);
        p.extend_from_slice(&ty.to_be_bytes());
        p.extend_from_slice(&0x8001u16.to_be_bytes());
        p.extend_from_slice(&120u32.to_be_bytes());
        p.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        p.extend_from_slice(rdata);
    }
    /// Answer of a scanner `instance` (PTR, SRV, TXT, A).
    pub fn response(service: &str, instance: &str, host: &str, ip: [u8; 4], port: u16, txt: &[&str]) -> Vec<u8> {
        let mut p = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3];
        let full = format!("{instance}.{service}");
        let mut ptr = Vec::new();
        name(&mut ptr, &full);
        rr(&mut p, service, super::T_PTR, &ptr);
        let mut srv = vec![0, 0, 0, 0];
        srv.extend_from_slice(&port.to_be_bytes());
        name(&mut srv, host);
        rr(&mut p, &full, super::T_SRV, &srv);
        let mut t = Vec::new();
        for s in txt {
            t.push(s.len() as u8);
            t.extend_from_slice(s.as_bytes());
        }
        rr(&mut p, &full, super::T_TXT, &t);
        rr(&mut p, host, super::T_A, &ip);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_shape() {
        let q = build_query();
        assert_eq!(&q[4..6], &[0, 2]);
        assert!(q.windows(6).any(|w| w == b"_uscan"));
        assert!(q.windows(7).any(|w| w == b"_uscans"));
    }

    #[test]
    fn parse_and_assemble() {
        let pkt = testpkt::response("_uscan._tcp.local", "HP OfficeJet Pro 9010 [ABC]", "HPABC.local", [192, 168, 1, 20], 8080, &["txtvers=1", "ty=HP OfficeJet Pro 9010 series", "rs=eSCL", "UUID=1234"]);
        let mut rec = Records::default();
        parse_response(&pkt, &mut rec).unwrap();
        let tls = testpkt::response("_uscans._tcp.local", "HP OfficeJet Pro 9010 [ABC]", "HPABC.local", [192, 168, 1, 20], 443, &["rs=/eSCL"]);
        parse_response(&tls, &mut rec).unwrap();
        let found = assemble(&rec, &HashMap::new());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].base_url, "https://192.168.1.20:443/eSCL/");
        assert!(found[0].tls);
        assert_eq!(found[1].base_url, "http://192.168.1.20:8080/eSCL/");
        assert_eq!(found[1].name, "HP OfficeJet Pro 9010 [ABC]");
        assert_eq!(found[1].model.as_deref(), Some("HP OfficeJet Pro 9010 series"));
        assert_eq!(found[1].uuid.as_deref(), Some("1234"));
    }

    #[test]
    fn hostile_packets_do_not_panic() {
        let mut rec = Records::default();
        assert!(parse_response(&[], &mut rec).is_none());
        // compression loop
        let loopy = [0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0xC0, 12, 0, 12, 0, 1, 0, 0, 0, 1, 0, 2, 0xC0, 12];
        let _ = parse_response(&loopy, &mut rec);
        // Truncated
        let pkt = testpkt::response("_uscan._tcp.local", "X", "x.local", [10, 0, 0, 1], 80, &[]);
        for n in 0..pkt.len() {
            let _ = parse_response(&pkt[..n], &mut Records::default());
        }
    }

    #[test]
    fn fallback_to_sender_ip() {
        let mut pkt = testpkt::response("_uscan._tcp.local", "Y", "y.local", [10, 0, 0, 9], 80, &[]);
        // remove the A record: counter to 2 additional
        pkt[11] = 2;
        let mut rec = Records::default();
        parse_response(&pkt, &mut rec).unwrap();
        rec.a.clear();
        let mut fb = HashMap::new();
        fb.insert("y._uscan._tcp.local".to_string(), "10.0.0.77".parse().unwrap());
        assert_eq!(assemble(&rec, &fb)[0].base_url, "http://10.0.0.77:80/eSCL/");
    }
}
