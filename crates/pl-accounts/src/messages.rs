//! Django-Messages (`FallbackStorage` → `CookieStorage`, Cookie `messages`).
//!
//! upstream: django/contrib/messages/storage/{base,cookie,fallback}.py@5.2. Encoding:
//! `TimestampSigner(key="django.http.cookies"+SECRET_KEY, salt="django.contrib.messages")
//! .sign_object("[" + ",".join(json.dumps(m, separators=(",", ":"))) + "]", compress=True)`,
//! message = `["__json_message", is_safe, level, text, extra_tags]`. allauth adds messages
//! with `extra_tags=""` (`DefaultAccountAdapter.add_message`). The session stage of the
//! fallback (only above 2048 bytes) is not reimplemented.

use serde_json::Value;

pub const COOKIE_NAME: &str = "messages";
const SALT: &str = "django.contrib.messages";
pub const SUCCESS: i64 = 25;
pub const INFO: i64 = 20;
pub const ERROR: i64 = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub level: i64,
    pub text: String,
    pub safe: bool,
    pub extra_tags: Option<String>,
}

impl Message {
    /// `Message.level_tag` (DEFAULT_TAGS).
    pub fn level_tag(&self) -> &'static str {
        match self.level {
            10 => "debug",
            20 => "info",
            25 => "success",
            30 => "warning",
            40 => "error",
            _ => "",
        }
    }
}

/// State of the messages of a request.
#[derive(Debug, Default)]
pub struct Messages {
    loaded: Vec<Message>,
    queued: Vec<Message>,
    /// Cookie had (valid) data → must be deleted when cleared.
    had_data: bool,
    /// Iterated (`used`).
    pub used: bool,
    added_new: bool,
}

fn key(secret: &str) -> String {
    format!("django.http.cookies{secret}")
}

impl Messages {
    pub fn load(secret: &str, cookie: Option<&str>) -> Self {
        let mut m = Messages::default();
        let Some(c) = cookie else { return m };
        match pl_authz::signing::loads(c, SALT, &key(secret)) {
            Ok(Value::Array(items)) => {
                for it in items {
                    if let Value::Array(a) = it
                        && a.first().and_then(|v| v.as_str()) == Some("__json_message")
                    {
                        m.loaded.push(Message {
                            safe: a.get(1).and_then(|v| v.as_i64()).unwrap_or(0) != 0,
                            level: a.get(2).and_then(|v| v.as_i64()).unwrap_or(20),
                            text: a.get(3).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            extra_tags: a.get(4).and_then(|v| v.as_str()).map(str::to_string),
                        });
                    }
                }
                m.had_data = !m.loaded.is_empty();
            }
            _ => {
                // bad signature: data marked as used so it gets removed
                m.used = true;
                m.had_data = true;
            }
        }
        m
    }

    /// `messages.add_message(request, level, text, extra_tags="")`.
    pub fn add(&mut self, level: i64, text: impl Into<String>) {
        self.added_new = true;
        self.queued.push(Message { level, text: text.into(), safe: false, extra_tags: Some(String::new()) });
    }

    /// Iteration in the template: all loaded + new messages, then `used`.
    pub fn iterate(&mut self) -> Vec<Message> {
        self.used = true;
        let mut all = std::mem::take(&mut self.loaded);
        all.append(&mut self.queued);
        all
    }

    /// `storage.update(response)` → `Set-Cookie` value or `None`.
    pub fn finish(&mut self, secret: &str, now: i64) -> Option<String> {
        let to_store: Vec<Message> = if self.used {
            std::mem::take(&mut self.queued)
        } else if self.added_new {
            let mut v = std::mem::take(&mut self.loaded);
            v.append(&mut self.queued);
            v
        } else {
            return None;
        };
        if to_store.is_empty() {
            return self
                .had_data
                .then(|| format!("{COOKIE_NAME}=\"\"; expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Path=/; SameSite=Lax"));
        }
        let parts: Vec<String> = to_store
            .iter()
            .map(|m| {
                let mut a = vec![
                    Value::from("__json_message"),
                    Value::from(if m.safe { 1 } else { 0 }),
                    Value::from(m.level),
                    Value::from(m.text.clone()),
                ];
                if let Some(t) = &m.extra_tags {
                    a.push(Value::from(t.clone()));
                }
                pl_authz::signing::python_json_dumps(&Value::Array(a))
            })
            .collect();
        let json = format!("[{}]", parts.join(","));
        let v = pl_authz::signing::dumps_at(&json, SALT, &key(secret), true, now);
        Some(format!("{COOKIE_NAME}={v}; HttpOnly; Path=/; SameSite=Lax"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";

    /// Cookie from Django (W3 reference) after alice's login.
    const DJANGO: &str = "W1siX19qc29uX21lc3NhZ2UiLDAsMjUsIlN1Y2Nlc3NmdWxseSBzaWduZWQgaW4gYXMgYWxpY2UuIiwiIl1d:1xPwp9:HWxE99K6F8IkA1qbAECgq2bjfx7V3Jq9DugnC9Yp-YE";

    #[test]
    fn roundtrip_with_django() {
        let mut m = Messages::load(SECRET, Some(DJANGO));
        assert!(m.had_data);
        let ts = pl_authz::signing::b62_decode("1xPwp9").unwrap();
        let mut fresh = Messages::default();
        fresh.add(SUCCESS, "Successfully signed in as alice.");
        let cookie = fresh.finish(SECRET, ts).unwrap();
        assert_eq!(cookie, format!("messages={DJANGO}; HttpOnly; Path=/; SameSite=Lax"));
        let all = m.iterate();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, "Successfully signed in as alice.");
        assert_eq!(all[0].level_tag(), "success");
        assert!(m.finish(SECRET, ts).unwrap().starts_with("messages=\"\";"));
    }

    #[test]
    fn untouched_without_new() {
        let mut m = Messages::load(SECRET, Some(DJANGO));
        assert_eq!(m.finish(SECRET, 0), None);
        let mut e = Messages::load(SECRET, None);
        e.iterate();
        assert_eq!(e.finish(SECRET, 0), None);
    }
}
