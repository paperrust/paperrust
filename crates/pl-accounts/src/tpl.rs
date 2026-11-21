//! PaperRust's own templates for the account pages (PaperRust design, minijinja).
//!
//! No template comes from paperless-ngx or django-allauth. Only the outward-facing
//! interfaces stayed the same: URLs, form methods and targets, field names (`login`, `password`,
//! `username`, `email`, `password1`, `password2`, `code`, `next`, `csrfmiddlewaretoken`),
//! status codes and script redirects (first install, `REDIRECT_LOGIN_TO_SSO`).
//! Inventory and differences: docs/contract/pl-accounts.md ("Own account pages").
//!
//! Texts: `t("auth.key", NAME=value)` from the own catalog (`crate::i18n`); catalog text and
//! inserted values are HTML-escaped. Output escaping like `django.utils.html.escape`
//! (`& < > " '`).

use std::sync::LazyLock;

use minijinja::value::{Kwargs, Value};
use minijinja::{AutoEscape, Environment, Error, ErrorKind, State};

use crate::i18n;

/// `django.utils.html.escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// Only still for pl-server (`frontend::render_index`, old index page with upstream texts):
/// replaces the upstream product name with `site_name`. The account pages no longer need this.
/// CORE-REQUEST: move to pl-server, then delete here.
pub fn with_site_name(text: &str, site_name: &str) -> String {
    const UPSTREAM_NAME: &str = "Paperless-ngx";
    text.replace(UPSTREAM_NAME, site_name)
}

pub(crate) const TEMPLATES: &[(&str, &str)] = &[
    ("layout.html", include_str!("../templates/layout.html")),
    ("providers.html", include_str!("../templates/providers.html")),
    ("login.html", include_str!("../templates/login.html")),
    ("signup.html", include_str!("../templates/signup.html")),
    ("account_inactive.html", include_str!("../templates/account_inactive.html")),
    ("password_reset.html", include_str!("../templates/password_reset.html")),
    ("password_reset_done.html", include_str!("../templates/password_reset_done.html")),
    ("password_reset_from_key.html", include_str!("../templates/password_reset_from_key.html")),
    ("password_reset_from_key_done.html", include_str!("../templates/password_reset_from_key_done.html")),
    ("mfa_authenticate.html", include_str!("../templates/mfa_authenticate.html")),
    ("social_authentication_error.html", include_str!("../templates/social_authentication_error.html")),
    ("social_login.html", include_str!("../templates/social_login.html")),
    ("social_signup.html", include_str!("../templates/social_signup.html")),
    ("social_login_cancelled.html", include_str!("../templates/social_login_cancelled.html")),
    ("signup_closed.html", include_str!("../templates/signup_closed.html")),
    ("email_confirm.html", include_str!("../templates/email_confirm.html")),
    ("verification_sent.html", include_str!("../templates/verification_sent.html")),
    ("csrf_403.html", include_str!("../templates/csrf_403.html")),
];

fn lang_of(state: &State) -> String {
    state.lookup("LANG").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "en-us".into())
}

/// Catalog text with inserted values, everything escaped (for templates).
fn translate(lang: &str, key: &str, kwargs: &Kwargs) -> Result<String, Error> {
    let text = i18n::t(lang, key);
    let mut out = escape(text);
    for k in kwargs.args() {
        let v: Value = kwargs.get(k)?;
        let s = if v.is_safe() { v.to_string() } else { escape(&v.to_string()) };
        out = out.replace(&format!("{{${k}}}"), &s);
    }
    Ok(out)
}

static ENV: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.set_formatter(|out, _state, value| {
        if value.is_undefined() || value.is_none() {
            return Ok(());
        }
        let s = value.to_string();
        if value.is_safe() {
            out.write_str(&s).map_err(|_| Error::new(ErrorKind::WriteFailure, "write"))
        } else {
            out.write_str(&escape(&s)).map_err(|_| Error::new(ErrorKind::WriteFailure, "write"))
        }
    });
    env.add_function("t", |state: &State, key: String, kwargs: Kwargs| -> Result<Value, Error> {
        let s = translate(&lang_of(state), &key, &kwargs)?;
        kwargs.assert_all_used()?;
        Ok(Value::from_safe_string(s))
    });
    env.add_function("html_lang", |state: &State| -> Value { Value::from(i18n::html_lang(&lang_of(state))) });
    for (name, src) in TEMPLATES {
        if let Err(e) = env.add_template(name, src) {
            tracing::error!(target: "pl_accounts", "template {name}: {e}");
        }
    }
    env
});

/// Renders a template with context `ctx` (must contain `LANG`, optionally `SITE_NAME`).
pub fn render(name: &str, ctx: Value) -> Result<String, Error> {
    ENV.get_template(name)?.render(ctx)
}

#[cfg(test)]
mod tests {
    use minijinja::context;

    fn base(lang: &str) -> minijinja::Value {
        context! {
            LANG => lang, SITE_NAME => "PaperRust", APP_TITLE => "PaperRust", APP_LOGO => "/__rs/brand/logo.svg",
            csrf_input => minijinja::Value::from_safe_string("<input type=\"hidden\" name=\"csrfmiddlewaretoken\" value=\"T\">".into()),
            GET => context! {}, messages => Vec::<minijinja::Value>::new(), form_errors => Vec::<minijinja::Value>::new(),
            EMAIL_ENABLED => true, ACCOUNT_ALLOW_SIGNUPS => true, signup_url => "/accounts/signup/", login_url => "/accounts/login/",
            urls => context! { account_login => "/accounts/login/", account_logout => "/accounts/logout/", account_signup => "/accounts/signup/", account_reset_password => "/accounts/password/reset/" },
            socialaccount_providers => Vec::<minijinja::Value>::new(),
        }
    }

    fn with(base: minijinja::Value, extra: minijinja::Value) -> minijinja::Value {
        minijinja::Value::from_iter(base.try_iter().unwrap().chain(extra.try_iter().unwrap()).map(|k| {
            let v = extra.get_item(&k).ok().filter(|v| !v.is_undefined()).unwrap_or_else(|| base.get_item(&k).unwrap());
            (k.to_string(), v)
        }))
    }

    /// Context with which every template can be rendered.
    fn full(lang: &str) -> minijinja::Value {
        with(
            base(lang),
            context! {
                form => context! { email => context! { name => "email", value => "" }, username => context! { name => "username", value => "" }, password1 => context! { name => "password1" }, password2 => context! { name => "password2" } },
                provider => context! { name => "Fake IdP" }, account => context! { get_provider => context! { name => "Fake IdP" } },
                confirmation => true, can_confirm => true, email => "a@example.org", user_display => "alice", action_url => "/accounts/confirm-email/k/",
                redirect_field => minijinja::Value::from_safe_string(String::new()),
                title => "x", main => "y",
            },
        )
    }

    #[test]
    fn django_escape() {
        assert_eq!(super::escape("a&b<c>\"d'/"), "a&amp;b&lt;c&gt;&quot;d&#x27;/");
    }

    #[test]
    fn all_templates_render_in_every_language() {
        for loc in crate::i18n::LOCALES {
            for (name, _) in super::TEMPLATES {
                let html = super::render(name, full(&loc.to_lowercase())).unwrap_or_else(|e| panic!("{name} ({loc}): {e:#}"));
                // No key was left untranslated.
                assert!(!html.contains("auth."), "{name} ({loc}) has an untranslated key");
            }
        }
    }

    #[test]
    fn layout_title_logo_footer() {
        let html = super::render("login.html", base("de-de")).unwrap();
        assert!(html.contains("<html lang=\"de\""));
        assert!(html.contains("<title>Anmelden · PaperRust</title>"), "{html}");
        assert!(html.contains("<img class=\"brand-logo\" src=\"/__rs/brand/logo.svg\""));
        assert!(html.contains("basiert auf paperless-ngx, GPL-3.0"));
        assert!(!html.contains("<source"));
        let dark = with(base("en-us"), context! { APP_LOGO_DARK => "/__rs/brand/logo-inverse.svg" });
        assert!(
            super::render("login.html", dark)
                .unwrap()
                .contains("<source media=\"(prefers-color-scheme: dark)\" srcset=\"/__rs/brand/logo-inverse.svg\">")
        );
        // Without logo: title as text
        let ctx = with(base("en-us"), context! { APP_LOGO => (), APP_TITLE => "Archiv <A&B>" });
        let html = super::render("login.html", ctx).unwrap();
        assert!(html.contains("<span class=\"brand-name\">Archiv &lt;A&amp;B&gt;</span>"));
        assert!(!html.contains("class=\"brand-logo\""));
        assert!(html.contains("based on paperless-ngx, GPL-3.0"));
    }

    /// Field names, form targets and redirects that login, API and links rely on.
    #[test]
    fn login_form_interface() {
        let html = super::render("login.html", base("en-us")).unwrap();
        for needle in [
            "<form id=\"form-account\" method=\"post\">",
            "name=\"csrfmiddlewaretoken\"",
            "name=\"login\"",
            "name=\"password\"",
            "href=\"/accounts/password/reset/\"",
            "href=\"/accounts/signup/\"",
        ] {
            assert!(html.contains(needle), "{needle}");
        }
        let first = with(base("en-us"), context! { FIRST_INSTALL => true });
        assert!(super::render("login.html", first).unwrap().contains("window.location.href = \"/accounts/signup/\""));
        let sso = with(
            base("en-us"),
            context! { DISABLE_REGULAR_LOGIN => true, REDIRECT_LOGIN_TO_SSO => true, socialaccount_providers => vec![context! { id => "openid_connect", name => "Fake IdP", href => "/accounts/oidc/f/login/?process=" }] },
        );
        let html = super::render("login.html", sso).unwrap();
        assert!(!html.contains("name=\"password\""));
        assert!(html.contains("<form id=\"social-login\" method=\"post\" action=\"/accounts/oidc/f/login/?process=\">"));
        assert!(html.contains("form.submit()"));
    }

    #[test]
    fn errors_messages_and_share_links() {
        let ctx = with(
            base("en-us"),
            context! { form_errors => vec![vec!["Wrong <b>"]], messages => vec![context! { level_tag => "success", text => "Signed in as a&b." }] },
        );
        let html = super::render("login.html", ctx).unwrap();
        assert!(html.contains("Wrong &lt;b&gt;"));
        assert!(html.contains("class=\"note note-error\" role=\"alert\""));
        assert!(html.contains("class=\"note note-success\""));
        assert!(html.contains("Signed in as a&amp;b."));
        let ctx = with(base("fr-fr"), context! { GET => context! { sharelink_expired => "1" } });
        assert!(super::render("login.html", ctx).unwrap().contains("Ce lien de partage a expiré."));
    }

    #[test]
    fn placeholders_are_escaped() {
        let ctx = with(full("en-us"), context! { provider => context! { name => "<script>x</script>" } });
        let html = super::render("social_login.html", ctx).unwrap();
        assert!(html.contains("from &lt;script&gt;x&lt;/script&gt;."));
        assert!(!html.contains("<script>x"));
    }

    /// No template mentions paperless-ngx/allauth – except the license notice (catalog key).
    #[test]
    fn templates_have_no_upstream_names() {
        for (name, src) in super::TEMPLATES {
            // Only exception: the CSS variables that pl-server inserts for the accent color
            // (pl-server/src/branding.rs `css_accounts`) are still called `--pngx-primary*`.
            let low = src.to_lowercase().replace("--pngx-primary-hover", "").replace("--pngx-primary", "");
            assert!(!low.contains("paperless") && !low.contains("allauth") && !low.contains("ngx") && !low.contains("pngx"), "{name}");
        }
        // Rendered page: exactly one mention, and that in the footer.
        let html = super::render("login.html", base("en-us")).unwrap();
        assert_eq!(html.matches("paperless-ngx").count(), 1);
        assert!(html.contains("<footer class=\"foot\">based on paperless-ngx, GPL-3.0</footer>"));
    }

    /// Every catalog key is used (templates or source code).
    #[test]
    fn no_unused_catalog_keys() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hay: String = super::TEMPLATES.iter().map(|(_, s)| *s).collect();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            hay.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
        }
        for k in crate::i18n::catalog("en-US").unwrap().keys() {
            assert!(hay.contains(&format!("\"{k}\"")), "unused key {k}");
        }
    }
}
