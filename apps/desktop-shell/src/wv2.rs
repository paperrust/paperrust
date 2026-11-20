//! WebView2 binding (Windows only), spike P1 (REMOTE-APP §4, §7):
//!
//! - **`ServerCertificateErrorDetected`** (`ICoreWebView2_14`): certificate errors of a server.
//!   The shell allows the connection (`ALWAYS_ALLOW`) only if the request goes to the origin of the
//!   active profile **and** the SPKI-SHA-256 of the presented certificate matches the confirmed
//!   `tls_pin`; expired/revoked certificates never. Otherwise default behavior
//!   (error page of WebView2). A self-signed certificate thus needs **no**
//!   entry in the user's certificate store, and the exception applies only in this window.
//! - **`NavigationCompleted`** with `IsSuccess = false` and connection error → immediate
//!   offline check (instead of the WebView2 error page, the own offline page).

use std::sync::Arc;

/// Decision about a certificate: (request URL, SPKI pin of the certificate, expired/revoked).
pub type CertCheck = Arc<dyn Fn(&str, &str, bool) -> bool + Send + Sync>;
/// Main page could not be loaded (connection error).
pub type NavFailed = Arc<dyn Fn() + Send + Sync>;

#[cfg(windows)]
pub fn install(window: &tauri::WebviewWindow, cert: CertCheck, nav_failed: NavFailed) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        NavigationCompletedEventHandler, ServerCertificateErrorDetectedEventHandler, take_pwstr,
    };
    use windows::core::{BOOL, Interface, PWSTR};

    let r = window.with_webview(move |pw| unsafe {
        let Ok(core) = pw.controller().CoreWebView2() else {
            tracing::warn!("WebView2: CoreWebView2 nicht verfügbar");
            return;
        };
        match core.cast::<ICoreWebView2_14>() {
            Ok(core14) => {
                let handler = ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_wv, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut uri = PWSTR::null();
                    args.RequestUri(&mut uri)?;
                    let uri = take_pwstr(uri);
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    args.ErrorStatus(&mut status)?;
                    let hard = status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
                        || status == COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED;
                    let certificate = args.ServerCertificate()?;
                    let mut pem = PWSTR::null();
                    certificate.ToPemEncoding(&mut pem)?;
                    let pem = take_pwstr(pem);
                    let pin = crate::remote::pem_to_der(&pem)
                        .and_then(|d| crate::remote::spki_pin(&d))
                        .unwrap_or_default();
                    let allow = !pin.is_empty() && cert(&uri, &pin, hard);
                    tracing::info!(%uri, status = status.0, allow, "WebView2: Zertifikatsfehler");
                    args.SetAction(if allow {
                        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW
                    } else {
                        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_DEFAULT
                    })?;
                    Ok(())
                }));
                let mut token = 0i64;
                if let Err(e) = core14.add_ServerCertificateErrorDetected(&handler, &mut token) {
                    tracing::warn!(error = %e, "WebView2: ServerCertificateErrorDetected nicht verfügbar");
                }
            }
            Err(e) => tracing::warn!(error = %e, "WebView2 zu alt für ServerCertificateErrorDetected (ICoreWebView2_14)"),
        }
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_wv, args| {
            let Some(args) = args else { return Ok(()) };
            let mut ok = BOOL::default();
            args.IsSuccess(&mut ok)?;
            if !ok.as_bool() {
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                args.WebErrorStatus(&mut status)?;
                let connection = [
                    COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED,
                    COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_RESET,
                    COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED,
                    COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
                    COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE,
                    COREWEBVIEW2_WEB_ERROR_STATUS_TIMEOUT,
                ]
                .contains(&status);
                tracing::info!(status = status.0, connection, "WebView2: Seite nicht geladen");
                if connection {
                    nav_failed();
                }
            }
            Ok(())
        }));
        let mut token = 0i64;
        if let Err(e) = core.add_NavigationCompleted(&handler, &mut token) {
            tracing::warn!(error = %e, "WebView2: NavigationCompleted");
        }
    });
    if let Err(e) = r {
        tracing::warn!(error = %e, "WebView2-Anbindung fehlgeschlagen");
    }
}

#[cfg(not(windows))]
pub fn install(_window: &tauri::WebviewWindow, _cert: CertCheck, _nav_failed: NavFailed) {
    // WebKit (macOS/Linux): no certificate exception without a keychain; instructions in the dialog.
}
