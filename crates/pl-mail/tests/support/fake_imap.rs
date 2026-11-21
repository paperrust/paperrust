//! Local fake IMAP server for the mail worker tests (127.0.0.1 only).
//!
//! Supports what imaplib/imap_tools and pl-mail send on fetch: CAPABILITY, LOGIN,
//! AUTHENTICATE PLAIN/XOAUTH2, SELECT, STATUS, LIST, UID SEARCH (subset of the criteria),
//! UID FETCH (BODY[]/BODY.PEEK[], UID, FLAGS, RFC822.SIZE), UID STORE ±FLAGS / +X-GM-LABELS,
//! UID COPY, UID MOVE, EXPUNGE, NOOP, LOGOUT. Every command is recorded (without tag).

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Msg {
    pub uid: u32,
    pub flags: BTreeSet<String>,
    pub labels: BTreeSet<String>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Folder {
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub msgs: Vec<Msg>,
}

#[derive(Debug, Default)]
pub struct State {
    pub folders: BTreeMap<String, Folder>,
    /// (connection, command without tag)
    pub log: Vec<(usize, String)>,
    pub connections: usize,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub capabilities: String,
    pub user: String,
    pub pass: String,
}

impl Default for Config {
    fn default() -> Self {
        Self { capabilities: "IMAP4rev1 AUTH=PLAIN MOVE".into(), user: "user".into(), pass: "pass".into() }
    }
}

pub type Shared = Arc<Mutex<State>>;

/// (uid, flags, Gmail labels)
pub type MsgSummary = (u32, Vec<String>, Vec<String>);

impl State {
    pub fn add_folder(&mut self, name: &str, uidvalidity: u32) {
        self.folders.insert(name.into(), Folder { uidvalidity, uidnext: 1, msgs: Vec::new() });
    }
    pub fn append(&mut self, folder: &str, raw: Vec<u8>, flags: &[&str]) -> u32 {
        let f = self.folders.get_mut(folder).expect("folder");
        let uid = f.uidnext;
        f.uidnext += 1;
        f.msgs.push(Msg { uid, flags: flags.iter().map(|s| s.to_string()).collect(), labels: BTreeSet::new(), raw });
        uid
    }
    /// Compact view: folder → [(uid, flags, labels)].
    pub fn summary(&self) -> BTreeMap<String, Vec<MsgSummary>> {
        self.folders
            .iter()
            .map(|(n, f)| {
                (n.clone(), f.msgs.iter().map(|m| (m.uid, m.flags.iter().cloned().collect(), m.labels.iter().cloned().collect())).collect())
            })
            .collect()
    }
}

/// Starts the server; returns the port.
pub async fn start(state: Shared, cfg: Config) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { return };
            let st = state.clone();
            let cfg = cfg.clone();
            let conn_no = {
                let mut s = st.lock().unwrap();
                s.connections += 1;
                s.connections
            };
            tokio::spawn(async move {
                let _ = serve(sock, st, cfg, conn_no).await;
            });
        }
    });
    port
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\")
    } else {
        s.to_string()
    }
}

/// Tokenizer for search criteria and arguments: atoms, "strings", ( ).
fn tokens(s: &str) -> Vec<String> {
    let b: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == ' ' {
            i += 1;
        } else if c == '(' || c == ')' {
            out.push(c.to_string());
            i += 1;
        } else if c == '"' {
            let mut t = String::from("\"");
            i += 1;
            while i < b.len() {
                if b[i] == '\\' && i + 1 < b.len() {
                    t.push(b[i]);
                    t.push(b[i + 1]);
                    i += 2;
                    continue;
                }
                t.push(b[i]);
                if b[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push(t);
        } else {
            let mut t = String::new();
            while i < b.len() && b[i] != ' ' && b[i] != '(' && b[i] != ')' {
                t.push(b[i]);
                i += 1;
            }
            out.push(t);
        }
    }
    out
}

fn header(raw: &[u8], name: &str) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut val = String::new();
    let mut found = false;
    for line in text.split('\n') {
        let l = line.trim_end_matches('\r');
        if l.is_empty() {
            break;
        }
        if found && (l.starts_with(' ') || l.starts_with('\t')) {
            val.push_str(l);
            continue;
        }
        if found {
            break;
        }
        if let Some((n, v)) = l.split_once(':')
            && n.eq_ignore_ascii_case(name)
        {
            val = v.trim().to_string();
            found = true;
        }
    }
    val
}

fn body(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match text.find("\r\n\r\n").map(|p| p + 4).or_else(|| text.find("\n\n").map(|p| p + 2)) {
        Some(p) => text[p..].to_string(),
        None => String::new(),
    }
}

fn mail_date(raw: &[u8]) -> Option<chrono::NaiveDate> {
    chrono::DateTime::parse_from_rfc2822(&header(raw, "date")).ok().map(|d| d.date_naive())
}

fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%d-%b-%Y").ok()
}

/// Evaluate criteria (AND over all keys; NOT, parentheses).
fn matches(t: &[String], i: &mut usize, m: &Msg) -> bool {
    let tok = t[*i].to_uppercase();
    *i += 1;
    let lc = |s: &str| s.to_lowercase();
    match tok.as_str() {
        "(" => {
            let mut ok = true;
            while *i < t.len() && t[*i] != ")" {
                ok &= matches(t, i, m);
            }
            *i += 1;
            ok
        }
        "NOT" => !matches(t, i, m),
        "ALL" => true,
        "SEEN" => m.flags.contains("\\Seen"),
        "UNSEEN" => !m.flags.contains("\\Seen"),
        "FLAGGED" => m.flags.contains("\\Flagged"),
        "UNFLAGGED" => !m.flags.contains("\\Flagged"),
        "KEYWORD" | "UNKEYWORD" => {
            let k = t[*i].clone();
            *i += 1;
            let has = m.flags.contains(&k);
            if tok == "KEYWORD" { has } else { !has }
        }
        "X-GM-LABELS" => {
            let k = unquote(&t[*i]);
            *i += 1;
            m.labels.contains(&k)
        }
        "FROM" | "TO" | "SUBJECT" | "BODY" => {
            let v = lc(&unquote(&t[*i]));
            *i += 1;
            let hay = match tok.as_str() {
                "FROM" => header(&m.raw, "from"),
                "TO" => header(&m.raw, "to"),
                "SUBJECT" => header(&m.raw, "subject"),
                _ => body(&m.raw),
            };
            lc(&hay).contains(&v)
        }
        "SINCE" => {
            let d = parse_date(&t[*i]);
            *i += 1;
            match (mail_date(&m.raw), d) {
                (Some(md), Some(d)) => md >= d,
                _ => true,
            }
        }
        _ => true,
    }
}

fn uid_set(s: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for part in s.split(',') {
        if let Some((a, b)) = part.split_once(':') {
            let a: u32 = a.parse().unwrap_or(0);
            let b: u32 = if b == "*" { u32::MAX } else { b.parse().unwrap_or(0) };
            for u in a.min(b)..=a.max(b).min(100_000) {
                out.insert(u);
            }
        } else if let Ok(u) = part.parse() {
            out.insert(u);
        }
    }
    out
}

fn flag_list(toks: &[String]) -> Vec<String> {
    toks.iter().filter(|t| *t != "(" && *t != ")").cloned().collect()
}

async fn serve(sock: tokio::net::TcpStream, st: Shared, cfg: Config, conn_no: usize) -> std::io::Result<()> {
    let (mut r, mut w) = sock.into_split();
    w.write_all(b"* OK fake IMAP ready\r\n").await?;
    let mut buf: Vec<u8> = Vec::new();
    let mut selected: Option<String> = None;
    let mut deleted_pending = false;
    loop {
        // read line
        let line = loop {
            if let Some(p) = buf.windows(2).position(|x| x == b"\r\n") {
                let l = String::from_utf8_lossy(&buf[..p]).into_owned();
                buf.drain(..p + 2);
                break l;
            }
            let mut chunk = [0u8; 8192];
            let n = r.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let tag = tag.to_string();
        let (cmd, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let cmd = cmd.to_uppercase();
        let logged = if cmd == "LOGIN" { "LOGIN <credentials>".to_string() } else { rest.to_string() };
        st.lock().unwrap().log.push((conn_no, logged));
        let mut out = String::new();
        let ok = |out: &mut String, text: &str| out.push_str(&format!("{tag} OK {text}\r\n"));
        match cmd.as_str() {
            "CAPABILITY" => {
                out.push_str(&format!("* CAPABILITY {}\r\n", cfg.capabilities));
                ok(&mut out, "CAPABILITY completed");
            }
            "NOOP" => ok(&mut out, "NOOP completed"),
            "LOGIN" => {
                let t = tokens(args);
                if t.len() == 2 && unquote(&t[0]) == cfg.user && unquote(&t[1]) == cfg.pass {
                    ok(&mut out, "LOGIN completed");
                } else {
                    out.push_str(&format!("{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n"));
                }
            }
            "AUTHENTICATE" => {
                w.write_all(b"+ \r\n").await?;
                let resp = loop {
                    if let Some(p) = buf.windows(2).position(|x| x == b"\r\n") {
                        let l = String::from_utf8_lossy(&buf[..p]).into_owned();
                        buf.drain(..p + 2);
                        break l;
                    }
                    let mut chunk = [0u8; 4096];
                    let n = r.read(&mut chunk).await?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                };
                let raw = base64::engine::general_purpose::STANDARD.decode(resp.trim()).unwrap_or_default();
                let good = match args.to_uppercase().as_str() {
                    "PLAIN" => raw == [b"\0".as_slice(), cfg.user.as_bytes(), b"\0", cfg.pass.as_bytes()].concat(),
                    "XOAUTH2" => raw == format!("user={}\x01auth=Bearer {}\x01\x01", cfg.user, cfg.pass).into_bytes(),
                    _ => false,
                };
                if good {
                    ok(&mut out, "AUTHENTICATE completed");
                } else {
                    out.push_str(&format!("{tag} NO AUTHENTICATE failed\r\n"));
                }
            }
            "SELECT" | "EXAMINE" => {
                let name = unquote(args);
                let s = st.lock().unwrap();
                match s.folders.get(&name) {
                    Some(f) => {
                        out.push_str(&format!(
                            "* {} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n",
                            f.msgs.len(),
                            f.uidvalidity
                        ));
                        out.push_str(&format!("{tag} OK [READ-WRITE] SELECT completed\r\n"));
                        selected = Some(name);
                    }
                    None => {
                        out.push_str(&format!("{tag} NO Mailbox does not exist\r\n"));
                        selected = None;
                    }
                }
            }
            "STATUS" => {
                let t = tokens(args);
                let name = unquote(&t[0]);
                let s = st.lock().unwrap();
                match s.folders.get(&name) {
                    Some(f) => {
                        out.push_str(&format!("* STATUS \"{name}\" (UIDVALIDITY {})\r\n", f.uidvalidity));
                        ok(&mut out, "STATUS completed");
                    }
                    None => out.push_str(&format!("{tag} NO no such mailbox\r\n")),
                }
            }
            "LIST" => {
                let s = st.lock().unwrap();
                for n in s.folders.keys() {
                    out.push_str(&format!("* LIST (\\HasNoChildren) \"/\" \"{n}\"\r\n"));
                }
                ok(&mut out, "LIST completed");
            }
            "EXPUNGE" => {
                if let Some(f) = &selected {
                    let mut s = st.lock().unwrap();
                    if let Some(fo) = s.folders.get_mut(f) {
                        fo.msgs.retain(|m| !m.flags.contains("\\Deleted"));
                    }
                }
                deleted_pending = false;
                ok(&mut out, "EXPUNGE completed");
            }
            "LOGOUT" => {
                w.write_all(format!("* BYE logging out\r\n{tag} OK LOGOUT completed\r\n").as_bytes()).await?;
                return Ok(());
            }
            "UID" => {
                let (sub, sargs) = args.split_once(' ').unwrap_or((args, ""));
                let sub = sub.to_uppercase();
                let Some(folder) = selected.clone() else {
                    w.write_all(format!("{tag} BAD no mailbox selected\r\n").as_bytes()).await?;
                    continue;
                };
                match sub.as_str() {
                    "SEARCH" => {
                        let mut t = tokens(sargs);
                        if t.first().is_some_and(|x| x.eq_ignore_ascii_case("CHARSET")) {
                            t.drain(..2);
                        }
                        let s = st.lock().unwrap();
                        let f = &s.folders[&folder];
                        let mut hits = Vec::new();
                        for m in &f.msgs {
                            let mut i = 0;
                            let mut ok_all = true;
                            while i < t.len() {
                                ok_all &= matches(&t, &mut i, m);
                            }
                            if ok_all {
                                hits.push(m.uid.to_string());
                            }
                        }
                        out.push_str(&format!("* SEARCH{}{}\r\n", if hits.is_empty() { "" } else { " " }, hits.join(" ")));
                        ok(&mut out, "SEARCH completed");
                    }
                    "FETCH" => {
                        let (set, _items) = sargs.split_once(' ').unwrap_or((sargs, ""));
                        let uids = uid_set(set);
                        let bytes = {
                            let s = st.lock().unwrap();
                            let f = &s.folders[&folder];
                            let mut bytes: Vec<u8> = Vec::new();
                            for (seq, m) in f.msgs.iter().enumerate() {
                                if !uids.contains(&m.uid) {
                                    continue;
                                }
                                let flags: Vec<String> = m.flags.iter().cloned().collect();
                                bytes.extend_from_slice(
                                    format!(
                                        "* {} FETCH (UID {} FLAGS ({}) RFC822.SIZE {} BODY[] {{{}}}\r\n",
                                        seq + 1,
                                        m.uid,
                                        flags.join(" "),
                                        m.raw.len(),
                                        m.raw.len()
                                    )
                                    .as_bytes(),
                                );
                                bytes.extend_from_slice(&m.raw);
                                bytes.extend_from_slice(b")\r\n");
                            }
                            bytes
                        };
                        w.write_all(&bytes).await?;
                        ok(&mut out, "FETCH completed");
                    }
                    "STORE" if tokens(sargs).len() < 3 => {
                        out.push_str(&format!(
                            "{tag} BAD Missing arguments
"
                        ));
                    }
                    "STORE" => {
                        let t = tokens(sargs);
                        let uids = uid_set(&t[0]);
                        let op = t[1].to_uppercase();
                        let vals = flag_list(&t[2..]);
                        let mut s = st.lock().unwrap();
                        let f = s.folders.get_mut(&folder).expect("folder");
                        for (seq, m) in f.msgs.iter_mut().enumerate() {
                            if !uids.contains(&m.uid) {
                                continue;
                            }
                            match op.as_str() {
                                "+FLAGS" => vals.iter().for_each(|v| {
                                    m.flags.insert(v.clone());
                                }),
                                "-FLAGS" => vals.iter().for_each(|v| {
                                    m.flags.remove(v);
                                }),
                                "+X-GM-LABELS" => vals.iter().for_each(|v| {
                                    m.labels.insert(unquote(v));
                                }),
                                _ => {}
                            }
                            let flags: Vec<String> = m.flags.iter().cloned().collect();
                            out.push_str(&format!("* {} FETCH (UID {} FLAGS ({}))\r\n", seq + 1, m.uid, flags.join(" ")));
                        }
                        if vals.iter().any(|v| v == "\\Deleted") {
                            deleted_pending = true;
                        }
                        ok(&mut out, "STORE completed");
                    }
                    "COPY" | "MOVE" => {
                        let t = tokens(sargs);
                        let uids = uid_set(&t[0]);
                        let dest = unquote(&t[1]);
                        let mut s = st.lock().unwrap();
                        if !s.folders.contains_key(&dest) {
                            out.push_str(&format!("{tag} NO [TRYCREATE] no such mailbox\r\n"));
                        } else {
                            let moving: Vec<Msg> = s.folders[&folder].msgs.iter().filter(|m| uids.contains(&m.uid)).cloned().collect();
                            for mut m in moving {
                                let d = s.folders.get_mut(&dest).expect("dest");
                                m.uid = d.uidnext;
                                d.uidnext += 1;
                                d.msgs.push(m);
                            }
                            if sub == "MOVE" {
                                s.folders.get_mut(&folder).expect("folder").msgs.retain(|m| !uids.contains(&m.uid));
                            }
                            ok(&mut out, &format!("{sub} completed"));
                        }
                    }
                    _ => out.push_str(&format!("{tag} BAD unknown UID command\r\n")),
                }
            }
            _ => out.push_str(&format!("{tag} BAD unknown command\r\n")),
        }
        let _ = deleted_pending;
        w.write_all(out.as_bytes()).await?;
    }
}
