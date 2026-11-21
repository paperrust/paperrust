//! Django password validation as configured by paperless (`AUTH_PASSWORD_VALIDATORS`) and the
//! "real password" rule of `PasswordValidationMixin`.
//!
//! upstream: src/paperless/serialisers.py@v3.4.2 (PasswordValidationMixin),
//! src/paperless/settings/__init__.py@v3.4.2 (AUTH_PASSWORD_VALIDATORS),
//! Django 5.2.18 django/contrib/auth/password_validation.py (validate_password,
//! UserAttributeSimilarityValidator, exceeds_maximum_length_ratio, MinimumLengthValidator,
//! CommonPasswordValidator, NumericPasswordValidator), difflib.SequenceMatcher.quick_ratio.
//! `data/common-passwords.txt.gz` is copied unchanged from Django 5.2.18 (BSD-3-Clause).

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::OnceLock;

const MAX_SIMILARITY: f64 = 0.7;
const MIN_LENGTH: usize = 8;

static COMMON_GZ: &[u8] = include_bytes!("../data/common-passwords.txt.gz");

fn common_passwords() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        let mut text = String::new();
        let mut gz = flate2::read::GzDecoder::new(COMMON_GZ);
        if gz.read_to_string(&mut text).is_err() {
            tracing::error!("common-passwords list could not be decoded");
            return HashSet::new();
        }
        text.lines().map(|l| py_strip(l).to_string()).collect()
    })
}

/// `PasswordValidationMixin._has_real_password`: non-empty and not only `*`.
pub fn has_real_password(value: &str) -> bool {
    !value.is_empty() && !value.replace('*', "").is_empty()
}

/// User attributes as seen by `UserAttributeSimilarityValidator` (values *before* the update).
#[derive(Debug, Clone, Default)]
pub struct UserAttrs {
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
}

/// Python `str.strip()` whitespace.
fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `re.split(r"\W+", value)` (Unicode word characters = alphanumeric or `_`).
fn split_non_word(value: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut in_sep = false;
    for c in value.chars() {
        let word = c.is_alphanumeric() || c == '_';
        if word {
            if in_sep {
                parts.push(String::new());
                in_sep = false;
            }
            if let Some(last) = parts.last_mut() {
                last.push(c);
            }
        } else {
            in_sep = true;
        }
    }
    if in_sep {
        parts.push(String::new());
    }
    parts
}

/// `difflib.SequenceMatcher(a=a, b=b).quick_ratio()`.
fn quick_ratio(a: &[char], b: &[char]) -> f64 {
    let mut fullbcount: HashMap<char, i64> = HashMap::new();
    for c in b {
        *fullbcount.entry(*c).or_insert(0) += 1;
    }
    let mut avail: HashMap<char, i64> = HashMap::new();
    let mut matches = 0i64;
    for c in a {
        let numb = match avail.get(c) {
            Some(n) => *n,
            None => fullbcount.get(c).copied().unwrap_or(0),
        };
        avail.insert(*c, numb - 1);
        if numb > 0 {
            matches += 1;
        }
    }
    let length = a.len() + b.len();
    if length == 0 { 1.0 } else { 2.0 * matches as f64 / length as f64 }
}

fn exceeds_maximum_length_ratio(pwd_len: usize, value_len: usize) -> bool {
    let length_bound_similarity = MAX_SIMILARITY / 2.0 * pwd_len as f64;
    pwd_len >= 10 * value_len && (value_len as f64) < length_bound_similarity
}

/// Python `str.isdigit()` approximation (Unicode decimal digits).
fn py_isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || is_unicode_decimal(c))
}

fn is_unicode_decimal(c: char) -> bool {
    // Nd ranges commonly met (Arabic-Indic, Devanagari, fullwidth, superscripts are isdigit too)
    matches!(c,
        '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}' | '\u{0966}'..='\u{096F}' |
        '\u{FF10}'..='\u{FF19}' | '\u{00B2}' | '\u{00B3}' | '\u{00B9}' | '\u{2070}' |
        '\u{2074}'..='\u{2079}' | '\u{2080}'..='\u{2089}')
}

/// `validate_password(password, user)` with paperless' four validators.
/// Returns all error messages in validator order (empty = valid).
pub fn validate_password(password: &str, user: Option<&UserAttrs>) -> Vec<String> {
    let mut errors = Vec::new();

    // 1. UserAttributeSimilarityValidator
    if let Some(u) = user {
        let pw: Vec<char> = password.to_lowercase().chars().collect();
        let attrs = [(&u.username, "username"), (&u.first_name, "first name"), (&u.last_name, "last name"), (&u.email, "email address")];
        'outer: for (value, verbose) in attrs {
            if value.is_empty() {
                continue;
            }
            let lower = value.to_lowercase();
            let mut parts = split_non_word(&lower);
            parts.push(lower.clone());
            for part in parts {
                let pc: Vec<char> = part.chars().collect();
                if exceeds_maximum_length_ratio(pw.len(), pc.len()) {
                    continue;
                }
                if quick_ratio(&pw, &pc) >= MAX_SIMILARITY {
                    errors.push(format!("The password is too similar to the {verbose}."));
                    break 'outer;
                }
            }
        }
    }

    // 2. MinimumLengthValidator
    if password.chars().count() < MIN_LENGTH {
        errors.push(format!("This password is too short. It must contain at least {MIN_LENGTH} characters."));
    }

    // 3. CommonPasswordValidator
    if common_passwords().contains(py_strip(&password.to_lowercase())) {
        errors.push("This password is too common.".to_string());
    }

    // 4. NumericPasswordValidator
    if py_isdigit(password) {
        errors.push("This password is entirely numeric.".to_string());
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> UserAttrs {
        UserAttrs {
            username: "testuser".into(),
            first_name: "Maximilian".into(),
            last_name: "".into(),
            email: "max.power@example.com".into(),
        }
    }

    #[test]
    fn django_messages() {
        assert_eq!(
            validate_password("1234", None),
            vec![
                "This password is too short. It must contain at least 8 characters.".to_string(),
                "This password is too common.".to_string(),
                "This password is entirely numeric.".to_string(),
            ]
        );
        assert_eq!(validate_password("password", None), vec!["This password is too common.".to_string()]);
        assert!(validate_password("correct horse battery staple 77", Some(&user())).is_empty());
        assert_eq!(validate_password("testuser1", Some(&user())), vec!["The password is too similar to the username.".to_string()]);
        assert_eq!(validate_password("maxpower!", Some(&user())), vec!["The password is too similar to the email address.".to_string()]);
    }

    #[test]
    fn quick_ratio_like_difflib() {
        let a: Vec<char> = "abcd".chars().collect();
        let b: Vec<char> = "bcde".chars().collect();
        assert!((quick_ratio(&a, &b) - 0.75).abs() < 1e-12);
        assert_eq!(split_non_word("max.power@example.com"), vec!["max", "power", "example", "com"]);
        assert_eq!(split_non_word("a@"), vec!["a", ""]);
    }

    #[test]
    fn real_password() {
        assert!(!has_real_password(""));
        assert!(!has_real_password("**********"));
        assert!(has_real_password("**x**"));
    }
}
