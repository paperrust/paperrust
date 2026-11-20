"""Upstream v3.4.2 URL+method inventory vs. pl_server::all_routes() (list_routes output) and
results/parity/routes-all.toml. Prints a markdown table (docs/STANDALONE-GAPS.md, section 2).

usage: cargo run -p pl-server --example list_routes > routes.txt
       python -I tests/parity/standalone-gaps/inventory.py routes.txt results/parity/routes-all.toml
"""
import re
import sys

routes_txt, routes_all = sys.argv[1], sys.argv[2]
rust = {}
for line in open(routes_txt, encoding="utf-8"):
    rid, method, path = line.rstrip("\n").split("\t")
    rust[(method, re.sub(r"\{[^}*]+\}", "{}", re.sub(r"\{\*[^}]+\}", "{*}", path)))] = rid
active = {m.group(1) for m in re.finditer(r'^"([^"]+)"\s*=\s*"rust"', open(routes_all, encoding="utf-8").read(), re.M)}

MV = ["GET", "POST", "PUT", "PATCH", "DELETE"]
LIST = ["GET", "POST"]
DETAIL = ["GET", "PUT", "PATCH", "DELETE"]


def vs(prefix, view, list_m=LIST, detail_m=DETAIL):
    out = [(f"/api/{prefix}/", m, f"{view}.{'list' if m == 'GET' else 'create'}") for m in list_m]
    names = {"GET": "retrieve", "PUT": "update", "PATCH": "partial_update", "DELETE": "destroy"}
    out += [(f"/api/{prefix}/{{}}/", m, f"{view}.{names[m]}") for m in detail_m]
    return out


U = []  # (path, method, upstream view, note)
add = lambda p, m, v, n="": U.append((p, m, v, n))

# --- /api/ explicit paths (src/paperless/urls.py) --------------------------------------------
add("/api/", "GET", "RedirectView(schema/view/)")
for m in ("GET", "POST"):
    add("/api/auth/login/", m, "allauth LoginView")
    add("/api/auth/logout/", m, "allauth LogoutView")
add("/api/search/", "GET", "GlobalSearchView")
add("/api/search/autocomplete/", "GET", "SearchAutoCompleteView")
add("/api/statistics/", "GET", "StatisticsView")
for p, v in [("post_document", "PostDocumentView"), ("bulk_edit", "BulkEditView"), ("delete", "DeleteDocumentsView"),
             ("reprocess", "ReprocessDocumentsView"), ("rotate", "RotateDocumentsView"), ("merge", "MergeDocumentsView"),
             ("merge_as_versions", "MergeDocumentsAsVersionsView"), ("edit_pdf", "EditPdfDocumentsView"),
             ("remove_password", "RemovePasswordDocumentsView"), ("bulk_download", "BulkDownloadView"),
             ("selection_data", "SelectionDataView"), ("chat", "ChatStreamingView")]:
    add(f"/api/documents/{p}/", "POST", v)
add("/api/bulk_edit_objects/", "POST", "BulkEditObjectsView")
add("/api/remote_version/", "GET", "RemoteVersionView")
add("/api/ui_settings/", "GET", "UiSettingsView.get")
add("/api/ui_settings/", "POST", "UiSettingsView.post")
add("/api/token/", "POST", "PaperlessObtainAuthTokenView")
add("/api/profile/", "GET", "ProfileView.get")
add("/api/profile/", "PATCH", "ProfileView.patch")
add("/api/profile/generate_auth_token/", "POST", "GenerateAuthTokenView")
add("/api/profile/disconnect_social_account/", "POST", "DisconnectSocialAccountView")
add("/api/profile/social_account_providers/", "GET", "SocialAccountProvidersView")
for m in ("GET", "POST", "DELETE"):
    add("/api/profile/totp/", m, f"TOTPView.{m.lower()}")
add("/api/status/", "GET", "SystemStatusView")
add("/api/trash/", "GET", "TrashView.get")
add("/api/trash/", "POST", "TrashView.post")
add("/api/oauth/callback/", "GET", "OauthCallbackView (paperless_mail)")
add("/api/schema/", "GET", "SpectacularAPIView")
add("/api/schema/view/", "GET", "SpectacularSwaggerView")
add("/api/auth/headless/{*}", "*", "allauth.headless.urls (browser/app API)")

# --- DefaultRouter ---------------------------------------------------------------------------
U += [(p, m, v, "") for p, m, v in vs("correspondents", "CorrespondentViewSet")]
U += [(p, m, v, "") for p, m, v in vs("document_types", "DocumentTypeViewSet")]
U += [(p, m, v, "") for p, m, v in vs("documents", "UnifiedSearchViewSet", ["GET"], DETAIL)]
for a, ms in [("root", ["GET"]), ("metadata", ["GET"]), ("suggestions", ["GET"]), ("ai_suggestions", ["GET"]),
              ("preview", ["GET"]), ("thumb", ["GET"]), ("download", ["GET"]), ("notes", ["GET", "POST", "DELETE"]),
              ("share_links", ["GET"]), ("history", ["GET"]), ("email", ["POST"]), ("update_version", ["POST"])]:
    for m in ms:
        add(f"/api/documents/{{}}/{a}/", m, f"UnifiedSearchViewSet.{a}")
add("/api/documents/{}/versions/{}/", "DELETE", "UnifiedSearchViewSet.delete_version")
add("/api/documents/{}/versions/{}/", "PATCH", "UnifiedSearchViewSet.update_version_label")
add("/api/documents/email/", "POST", "UnifiedSearchViewSet.email_documents")
add("/api/documents/next_asn/", "GET", "UnifiedSearchViewSet.next_asn")
U += [(p, m, v, "") for p, m, v in vs("logs", "LogViewSet", ["GET"], ["GET"])]
U += [(p, m, v, "") for p, m, v in vs("tags", "TagViewSet")]
U += [(p, m, v, "") for p, m, v in vs("saved_views", "SavedViewViewSet")]
U += [(p, m, v, "") for p, m, v in vs("storage_paths", "StoragePathViewSet")]
add("/api/storage_paths/test/", "POST", "StoragePathViewSet.test")
U += [(p, m, v, "") for p, m, v in vs("tasks", "TasksViewSet (ReadOnly)", ["GET"], ["GET"])]
add("/api/tasks/acknowledge/", "POST", "TasksViewSet.acknowledge")
add("/api/tasks/summary/", "GET", "TasksViewSet.summary")
add("/api/tasks/status_counts/", "GET", "TasksViewSet.status_counts")
add("/api/tasks/active/", "GET", "TasksViewSet.active")
add("/api/tasks/run/", "POST", "TasksViewSet.run")
U += [(p, m, v, "") for p, m, v in vs("users", "UserViewSet")]
add("/api/users/{}/deactivate_totp/", "POST", "UserViewSet.deactivate_totp")
U += [(p, m, v, "") for p, m, v in vs("groups", "GroupViewSet")]
U += [(p, m, v, "") for p, m, v in vs("mail_accounts", "MailAccountViewSet")]
add("/api/mail_accounts/test/", "POST", "MailAccountViewSet.test")
add("/api/mail_accounts/{}/process/", "POST", "MailAccountViewSet.process")
U += [(p, m, v, "") for p, m, v in vs("mail_rules", "MailRuleViewSet")]
U += [(p, m, v, "") for p, m, v in vs("share_link_bundles", "ShareLinkBundleViewSet (get/post/delete)", LIST, ["GET", "DELETE"])]
add("/api/share_link_bundles/{}/rebuild/", "POST", "ShareLinkBundleViewSet.rebuild")
U += [(p, m, v, "") for p, m, v in vs("share_links", "ShareLinkViewSet (no update)", LIST, ["GET", "DELETE"])]
U += [(p, m, v, "") for p, m, v in vs("workflow_triggers", "WorkflowTriggerViewSet")]
U += [(p, m, v, "") for p, m, v in vs("workflow_actions", "WorkflowActionViewSet")]
U += [(p, m, v, "") for p, m, v in vs("workflows", "WorkflowViewSet")]
U += [(p, m, v, "") for p, m, v in vs("custom_fields", "CustomFieldViewSet")]
U += [(p, m, v, "") for p, m, v in vs("config", "ApplicationConfigurationViewSet")]
U += [(p, m, v, "") for p, m, v in vs("processed_mail", "ProcessedMailViewSet (ReadOnly)", ["GET"], ["GET"])]
add("/api/processed_mail/bulk_delete/", "POST", "ProcessedMailViewSet.bulk_delete")

# --- outside /api/ ---------------------------------------------------------------------------
add("/share/{}/", "GET", "SharedLinkView")
add("/share/{}", "GET", "SharedLinkView")
add("/favicon.ico", "GET", "FaviconView")
add("/admin/{*}", "*", "django.contrib.admin")
for k in ("doc", "thumb", "preview"):
    add(f"/fetch/{k}/{{}}", "GET", f"RedirectView(api/documents/<pk>/{'download' if k == 'doc' else k}/)")
add("/assets/{*}", "GET", "RedirectView(static/frontend/en-US/assets/)")
add("/logo/", "GET", "serve_logo")
add("/logo", "GET", "serve_logo")
add("/logo/{*}", "GET", "serve_logo")
for p, v, ms in [("login", "LoginView", ["GET", "POST"]), ("logout", "LogoutView", ["GET", "POST"]),
                 ("signup", "SignupView", ["GET", "POST"]), ("account_inactive", "AccountInactiveView", ["GET"]),
                 ("password/reset", "PasswordResetView", ["GET", "POST"]), ("password/reset/done", "PasswordResetDoneView", ["GET"]),
                 ("password/reset/key/done", "PasswordResetFromKeyDoneView", ["GET"]),
                 ("confirm-email/{}", "ConfirmEmailView", ["GET", "POST"]),
                 ("password/reset/key/{}", "PasswordResetFromKeyView", ["GET", "POST"]),
                 ("3rdparty/login/cancelled", "LoginCancelledView", ["GET"]), ("3rdparty/login/error", "LoginErrorView", ["GET"]),
                 ("3rdparty/signup", "socialaccount SignupView", ["GET", "POST"]),
                 ("oidc/{}/login", "openid_connect login", ["GET", "POST"]),
                 ("oidc/{}/login/callback", "openid_connect callback", ["GET", "POST"]),
                 ("2fa/authenticate", "mfa AuthenticateView", ["GET", "POST"])]:
    for m in ms:
        add(f"/accounts/{p}/", m, f"allauth {v}")
add("/accounts/<provider>/login/", "*", "allauth provider URLs of other installed social providers")
add("/", "GET", "IndexView (login_required; catch-all `.*`)")
add("/{*}", "GET", "IndexView catch-all (SPA deep links)")
add("/static/{*}", "GET", "WhiteNoise (STATIC_URL)")
add("/static/drf_spectacular_sidecar/swagger-ui-dist/{}", "GET", "WhiteNoise (swagger-ui sidecar)")
add("/ws/status/", "WS", "channels StatusConsumer")
add("(every DRF view)", "OPTIONS", "APIView.options (SimpleMetadata)")
add("(every DRF router route)", "*", "DefaultRouter format suffix `.json`/`.api`")

NOT_ROUTES = {
    "/{*}": "frontend fallback (pl-server frontend::fallback, `is_upstream_view`)",
    "/ws/status/": "pl-server ws.rs",
    "/static/{*}": "static.files",
}

rows = []
missing = 0
for p, m, v, n in U:
    rid = rust.get((m, p))
    if rid is None and p == "/share/{}/":
        rid = rust.get((m, "/share/{}/"))
    if rid is None and p == "/logo/{*}":
        rid = rust.get((m, "/logo/{*}"))
    if rid:
        state = "rust" if rid in active else "**not active in routes-all**"
        cell = f"`{rid}`" + ("" if rid in active else f" ({state})")
    elif p in NOT_ROUTES:
        cell = NOT_ROUTES[p]
    else:
        cell = "**MISSING**"
        missing += 1
    rows.append((p, m, v, cell, n))

print("| Endpoint | Methode | upstream View | Rust-Route / Status | Notiz |")
print("|---|---|---|---|---|")
for p, m, v, cell, n in rows:
    print(f"| `{p}` | {m} | {v} | {cell} | {n} |")
print()
print(f"<!-- upstream rows: {len(rows)}, missing: {missing} -->")
extra = sorted(set(rust.values()) - {r[3].strip('`').split('`')[0] for r in rows})
print("Rust-only routes (no upstream counterpart):", ", ".join(f"`{e}`" for e in extra))
