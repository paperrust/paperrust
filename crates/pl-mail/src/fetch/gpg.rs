//! GPG decryption of fetched mails (gap M1).
//!
//! upstream: src/paperless_mail/preprocessor.py@v3.4.2 (MailMessageDecryptor: able_to_run, run,
//! _to_email_message, _build_decrypted_message), src/paperless_mail/mail.py@v3.4.2
//! (_init_preprocessors, _preprocess_message, `_handle_message`),
//! src/paperless/settings/__init__.py@v3.4.2 (EMAIL_GNUPG_HOME = PAPERLESS_EMAIL_GNUPG_HOME,
//! EMAIL_ENABLE_GPG_DECRYPTOR = PAPERLESS_ENABLE_GPG_DECRYPTOR).
//!
//! Upstream calls the `gpg` program via python-gnupg (`gpg --batch … --decrypt`, input is
//! the whole message as text). We do exactly the same. Difference (deliberate): if
//! decryption is enabled but `gpg` is missing or `EMAIL_GNUPG_HOME` is not a directory,
//! upstream silently drops the preprocessor and consumes encrypted mails as ciphertext.
//! Here an encrypted mail is then skipped with a clear error message (it stays in the
//! mailbox and is retried on the next run).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

/// Maximum duration of a `gpg` call.
const GPG_TIMEOUT: Duration = Duration::from_secs(120);

/// `EMAIL_ENABLE_GPG_DECRYPTOR` + `EMAIL_GNUPG_HOME`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpgConfig {
    pub enabled: bool,
    pub home: Option<PathBuf>,
    /// Program (default `gpg`, like python-gnupg `gpgbinary`).
    pub binary: String,
}

impl GpgConfig {
    pub fn from_core(core: &pl_core::Config) -> Self {
        let enabled = match core.env.bool("PAPERLESS_ENABLE_GPG_DECRYPTOR", "NO") {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "paperless_mail", "PAPERRUST_ENABLE_GPG_DECRYPTOR: {e}; decryption disabled");
                false
            }
        };
        let home = core.env.get("PAPERLESS_EMAIL_GNUPG_HOME").map(PathBuf::from);
        Self { enabled, home, binary: "gpg".into() }
    }
}

/// Why the enabled decryption cannot run (`able_to_run`/`GPG()` error).
pub fn unavailable_reason(cfg: &GpgConfig) -> Option<String> {
    if let Some(h) = &cfg.home
        && !h.is_dir()
    {
        return Some(format!("EMAIL_GNUPG_HOME {} is not a directory", h.display()));
    }
    if !gpg_available(&cfg.binary) {
        return Some(format!("the '{}' program was not found (install GnuPG)", cfg.binary));
    }
    None
}

fn gpg_available(binary: &str) -> bool {
    static CHECK: OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    let m = CHECK.get_or_init(Default::default);
    if let Ok(g) = m.lock()
        && let Some(v) = g.get(binary)
    {
        return *v;
    }
    let ok = std::process::Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if let Ok(mut g) = m.lock() {
        g.insert(binary.to_string(), ok);
    }
    ok
}

/// `message.obj.get_content_type() == "multipart/encrypted"`.
pub fn is_encrypted(mail: &pl_parsers::mail::MailMessage) -> bool {
    mail.msg.get_content_type() == "multipart/encrypted"
}

/// A header line (incl. continuation lines) as raw bytes plus name/value.
struct RawHeader<'a> {
    name: String,
    value_empty: bool,
    raw: Vec<&'a [u8]>,
}

/// Header part like `message_from_bytes`: header lines up to the first blank line or the first line
/// that is not a header line. Returns: header lines, rest (body without separator line).
fn split_headers(data: &[u8]) -> (Vec<RawHeader<'_>>, &[u8]) {
    let mut headers: Vec<RawHeader<'_>> = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let end = data[pos..].iter().position(|b| *b == b'\n').map(|i| pos + i + 1).unwrap_or(data.len());
        let line = &data[pos..end];
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if content.is_empty() {
            return (headers, &data[end..]);
        }
        if matches!(content.first(), Some(b' ' | b'\t')) {
            if let Some(h) = headers.last_mut() {
                if content.iter().any(|b| !b.is_ascii_whitespace()) {
                    h.value_empty = false;
                }
                h.raw.push(line);
                pos = end;
                continue;
            }
            return (headers, &data[pos..]);
        }
        let Some(colon) = content.iter().position(|b| *b == b':') else {
            return (headers, &data[pos..]);
        };
        let name = String::from_utf8_lossy(&content[..colon]).trim().to_string();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return (headers, &data[pos..]);
        }
        let value_empty = content[colon + 1..].iter().all(|b| b.is_ascii_whitespace());
        headers.push(RawHeader { name, value_empty, raw: vec![line] });
        pos = end;
    }
    (headers, &data[data.len()..])
}

/// `_build_decrypted_message`: append header lines of the original message that the decrypted part
/// does not have (`if not decrypted_message.get(header): add_header(…)` – so per name only
/// the first occurrence).
pub fn merge_headers(original: &[u8], decrypted: &[u8]) -> Vec<u8> {
    let nl: &[u8] = if decrypted.windows(2).any(|w| w == b"\r\n") { b"\r\n" } else { b"\n" };
    let (dec_headers, body) = split_headers(decrypted);
    let (orig_headers, _) = split_headers(original);
    let mut present: Vec<(String, bool)> = dec_headers.iter().map(|h| (h.name.to_ascii_lowercase(), !h.value_empty)).collect();
    let mut out = Vec::with_capacity(decrypted.len() + original.len().min(64 * 1024));
    let push_line = |out: &mut Vec<u8>, line: &[u8]| {
        let c = line.strip_suffix(b"\n").unwrap_or(line);
        let c = c.strip_suffix(b"\r").unwrap_or(c);
        out.extend_from_slice(c);
        out.extend_from_slice(nl);
    };
    for h in &dec_headers {
        for l in &h.raw {
            push_line(&mut out, l);
        }
    }
    for h in &orig_headers {
        let key = h.name.to_ascii_lowercase();
        // decrypted_message.get(header) is "truthy" → do not add
        if present.iter().any(|(n, truthy)| *n == key && *truthy) {
            continue;
        }
        for l in &h.raw {
            push_line(&mut out, l);
        }
        present.push((key, !h.value_empty));
    }
    out.extend_from_slice(nl);
    out.extend_from_slice(body);
    out
}

/// `self._gpg.decrypt(email_message.as_string())`: plain text or error text
/// (`Decryption failed: <status>, <stderr>`).
pub async fn gpg_decrypt(cfg: &GpgConfig, raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut cmd = tokio::process::Command::new(&cfg.binary);
    cmd.args(["--status-fd", "2", "--no-tty", "--batch", "--yes", "--quiet"]);
    if let Some(h) = &cfg.home {
        // `--homedir .` in the directory itself: same effect as the full path, but also works
        // with the MSYS gpg of Git for Windows (which reads `C:/…` as a relative path).
        cmd.current_dir(h).arg("--homedir").arg(".");
    }
    cmd.arg("--decrypt").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("Decryption failed: cannot start {}: {e}", cfg.binary))?;
    let mut stdin = child.stdin.take().ok_or("Decryption failed: no stdin")?;
    let input = raw.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
        let _ = stdin.shutdown().await;
    });
    let out = match tokio::time::timeout(GPG_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("Decryption failed: {e}")),
        Err(_) => return Err("Decryption failed: gpg timed out".into()),
    };
    let _ = writer.await;
    let stderr = String::from_utf8_lossy(&out.stderr);
    // python-gnupg: `ok` only with [GNUPG:] DECRYPTION_OKAY; `status` = last status description
    let decryption_ok = stderr.lines().any(|l| l.starts_with("[GNUPG:] DECRYPTION_OKAY"));
    if out.status.success() && decryption_ok {
        return Ok(out.stdout);
    }
    let status = stderr
        .lines()
        .filter_map(|l| l.strip_prefix("[GNUPG:] "))
        .filter(|l| l.starts_with("DECRYPTION_FAILED") || l.starts_with("NO_SECKEY") || l.starts_with("NODATA") || l.starts_with("ERROR"))
        .map(status_text)
        .next_back()
        .unwrap_or_else(|| "decryption failed".into());
    let plain: Vec<&str> = stderr.lines().filter(|l| !l.starts_with("[GNUPG:]")).collect();
    Err(format!("Decryption failed: {status}, {}", plain.join("\n").trim()))
}

/// Status line → python-gnupg `status` text.
fn status_text(l: &str) -> String {
    let key = l.split_whitespace().next().unwrap_or("");
    match key {
        "NO_SECKEY" => "no secret key".into(),
        "NODATA" => "no data was provided".into(),
        "DECRYPTION_FAILED" => "decryption failed".into(),
        _ => l.to_ascii_lowercase(),
    }
}

/// `MailMessageDecryptor.run(message)`: `Ok(None)` = unchanged (not encrypted or
/// decryption off), `Ok(Some(raw))` = new message, `Err` = do not process.
pub async fn decrypt_if_needed(cfg: &GpgConfig, mail: &pl_parsers::mail::MailMessage, raw: &[u8]) -> Result<Option<Vec<u8>>, String> {
    if !cfg.enabled {
        return Ok(None);
    }
    if !is_encrypted(mail) {
        tracing::debug!(target: "paperless_mail_message_decryptor", "Message not encrypted. Keep unchanged");
        return Ok(None);
    }
    tracing::debug!(target: "paperless_mail_message_decryptor", "Message is encrypted.");
    if let Some(why) = unavailable_reason(cfg) {
        return Err(format!("Decryption failed: GPG decryption is enabled but {why}; the encrypted mail is not processed"));
    }
    let plain = gpg_decrypt(cfg, raw).await.inspect_err(|e| {
        tracing::debug!(target: "paperless_mail_message_decryptor", "Message decryption failed with status message {e}");
    })?;
    tracing::debug!(target: "paperless_mail_message_decryptor", "Message decrypted successfully.");
    Ok(Some(merge_headers(raw, &plain)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_first_of_each_missing_header() {
        let orig = b"Received: a\r\nReceived: b\r\nSubject: Hi\r\n there\r\nFrom: x@y\r\nContent-Type: multipart/encrypted; boundary=\"q\"\r\n\r\n--q--\r\n";
        let dec = b"Content-Type: text/plain\r\n\r\nhello\r\n";
        let out = String::from_utf8(merge_headers(orig, dec)).unwrap_or_default();
        assert_eq!(out, "Content-Type: text/plain\r\nReceived: a\r\nSubject: Hi\r\n there\r\nFrom: x@y\r\n\r\nhello\r\n");
    }

    #[test]
    fn disabled_or_plain_is_unchanged() {
        let mail = pl_parsers::mail::MailMessage::from_bytes(b"Content-Type: text/plain\n\nx", 1000);
        let rt = tokio::runtime::Builder::new_current_thread().build();
        let Ok(rt) = rt else { return };
        let on = GpgConfig { enabled: true, home: None, binary: "gpg".into() };
        assert_eq!(rt.block_on(decrypt_if_needed(&GpgConfig::default(), &mail, b"")), Ok(None));
        assert_eq!(rt.block_on(decrypt_if_needed(&on, &mail, b"")), Ok(None));
        let enc = pl_parsers::mail::MailMessage::from_bytes(b"Content-Type: multipart/encrypted; boundary=q\n\n--q--\n", 1000);
        let missing = GpgConfig { enabled: true, home: None, binary: "paperrust-no-such-gpg".into() };
        let r = rt.block_on(decrypt_if_needed(&missing, &enc, b""));
        assert!(r.as_ref().is_err_and(|e| e.contains("not found")), "{r:?}");
    }

    /// Real encryption and decryption with the `gpg` program (own keyring in a
    /// temp directory, key without passphrase).
    #[tokio::test]
    #[ignore = "braucht das Programm gpg (GnuPG) im PATH"]
    async fn roundtrip_with_gpg() {
        let home = std::env::temp_dir().join(format!("pl-mail-gpg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).expect("temp dir");
        let gen_key = std::process::Command::new("gpg")
            .current_dir(&home)
            .args(["--homedir", "."])
            .args(["--batch", "--passphrase", "", "--quick-gen-key", "paperrust-test@example.invalid", "default", "default", "never"])
            .output()
            .expect("gpg --quick-gen-key");
        assert!(gen_key.status.success(), "{}", String::from_utf8_lossy(&gen_key.stderr));
        let inner = "Content-Type: multipart/mixed; boundary=\"in\"\r\n\r\n--in\r\nContent-Type: text/plain\r\n\r\nSecret body\r\n--in\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"a.pdf\"\r\n\r\n%PDF-1.4 test\r\n--in--\r\n";
        let tmp_in = home.join("inner.eml");
        std::fs::write(&tmp_in, inner).expect("write");
        let enc = std::process::Command::new("gpg")
            .current_dir(&home)
            .args(["--homedir", "."])
            .args([
                "--batch",
                "--yes",
                "--armor",
                "--trust-model",
                "always",
                "-r",
                "paperrust-test@example.invalid",
                "--encrypt",
                "-o",
                "-",
            ])
            .arg("inner.eml")
            .output()
            .expect("gpg --encrypt");
        assert!(enc.status.success(), "{}", String::from_utf8_lossy(&enc.stderr));
        let armored = String::from_utf8_lossy(&enc.stdout);
        let outer = format!(
            "From: a@b.example\r\nSubject: Encrypted\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n\r\n--b\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\n\r\n{armored}\r\n--b--\r\n"
        );
        let mail = pl_parsers::mail::MailMessage::from_bytes(outer.as_bytes(), 1000);
        let cfg = GpgConfig { enabled: true, home: Some(home.clone()), binary: "gpg".into() };
        let r = decrypt_if_needed(&cfg, &mail, outer.as_bytes()).await;
        let _ = std::process::Command::new("gpgconf").current_dir(&home).args(["--homedir", ".", "--kill", "gpg-agent"]).output();
        let _ = std::fs::remove_dir_all(&home);
        let raw = r.ok().flatten().unwrap_or_default();
        let m = pl_parsers::mail::MailMessage::from_bytes(&raw, 1000);
        assert_eq!(m.subject(), "Encrypted");
        assert_eq!(m.attachments().len(), 1);
    }
}
