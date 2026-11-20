//! Shared helpers of the parity runs against Django.
//!
//! PaperRust renders the account pages with its own templates and texts (contract
//! docs/contract/pl-accounts.md, "Own account pages"). Therefore what is compared is no longer the
//! wording but the interface of a page: forms (method, target), field names,
//! error/message markers and script redirects.

#![allow(dead_code)]

/// Interface of an HTML page, independent of wording and design.
pub fn page_skeleton(html: &str) -> String {
    let attr = |tag: &str, name: &str| -> String {
        let re = regex::Regex::new(&format!(r#"(?i)\b{name}="([^"]*)""#)).unwrap();
        re.captures(tag).map(|c| c[1].to_string()).unwrap_or_default()
    };
    let mut out = Vec::new();
    let forms = regex::Regex::new(r"(?is)<form\b[^>]*>").unwrap();
    for m in forms.find_iter(html) {
        let method = attr(m.as_str(), "method").to_lowercase();
        out.push(format!("form {method} {}", attr(m.as_str(), "action")));
    }
    let mut names: Vec<String> = regex::Regex::new(r"(?is)<input\b[^>]*>")
        .unwrap()
        .find_iter(html)
        .map(|m| attr(m.as_str(), "name"))
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    out.push(format!("fields {}", names.join(",")));
    // Error in the form: Django `alert-danger`, PaperRust `note-error" role="alert"`
    let form_error = html.contains("alert alert-danger") || html.contains("note note-error\" role=\"alert\"");
    out.push(format!("form_error {form_error}"));
    // Messages (django.contrib.messages): levels in order
    let mut levels = Vec::new();
    for c in regex::Regex::new(r#"class="alert alert-(success|info|warning|error|debug)""#).unwrap().captures_iter(html) {
        levels.push(c[1].to_string());
    }
    for c in regex::Regex::new(r#"class="note note-(success|info|warning|error|debug)" role="status""#).unwrap().captures_iter(html) {
        levels.push(c[1].to_string());
    }
    out.push(format!("messages {}", levels.join(",")));
    for (what, needle) in [("redirect_signup", "window.location.href"), ("auto_submit", ".submit()")] {
        out.push(format!("{what} {}", html.contains(needle)));
    }
    out.join("\n")
}

/// Content of a (decoded) messages cookie without the texts: levels only.
pub fn message_levels(decoded: &str) -> String {
    let re = regex::Regex::new(r#"\["__json_message",\s*(\d),\s*(\d+),\s*"(?:[^"\\]|\\.)*""#).unwrap();
    re.replace_all(decoded, "[$1,$2").into_owned()
}

/// English catalog text (Rust side), for checking messages.
pub fn en(key: &str) -> &'static str {
    static EN: std::sync::LazyLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::LazyLock::new(|| serde_json::from_str(include_str!("../../i18n/en-US.json")).unwrap());
    EN.get(key).and_then(|v| v.as_str()).unwrap_or("<missing key>")
}

#[test]
fn skeleton_ignores_wording() {
    let django = r#"<form class="form-accounts" id="form-account" method="post"><input type="hidden" name="csrfmiddlewaretoken" value="x"><div class="alert alert-danger" role="alert">Bad</div><input type="text" name="login"><input type="password" name="password"></form>"#;
    let ours = r#"<form id="form-account" method="post"><div class="note note-error" role="alert"><p>Falsch</p></div><input type="hidden" name="csrfmiddlewaretoken" value="y"><input name="login"><input type="password" name="password"></form>"#;
    assert_eq!(page_skeleton(django), page_skeleton(ours));
    assert_ne!(page_skeleton(django), page_skeleton(r#"<form method="post"><input name="user"></form>"#));
    assert_eq!(message_levels(r#"[["__json_message",0,25,"Successfully signed in as \"a\"."]]"#), r#"[[0,25]]"#);
}
