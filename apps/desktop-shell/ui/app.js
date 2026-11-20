// Splash, first-start form (local or "Mit einem Server verbinden"), connection form,
// error, offline and SSO page. Polls the state of the shell (polling instead of events: nothing gets
// lost if the page loads later than the first event) and switches to the UI as soon as
// the server is ready (local: http://127.0.0.1:<port>/, remote: address of the profile).
"use strict";
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const $ = (id) => document.getElementById(id);
  const show = (el, on) => { el.hidden = !on; };
  const form = $("setup-form");
  let busy = false;
  let errorLog = null;
  // "first" = first start (local: three ways, or server), "setup" = choice of the supervisor,
  // "connect" = server connection only
  let formKind = null;
  // to be confirmed by the user: SPKI pin from the certificate dialog
  let pendingPin = "";
  let offlineTimer = null;
  // after a successful connect the shell restarts – no more switching
  let restarting = false;
  // reconciliation after the switch seen ("PaperRust öffnen" clicked)
  let switchSeen = false;
  let readyUrl = null;

  const VERDICT = {
    RUNNING: ["…", "Abgleich läuft", "PaperRust vergleicht die übernommenen Daten mit paperless-ngx. Du kannst PaperRust schon öffnen; den Bericht findest du danach im Log-Ordner."],
    OK: ["OK", "Alles übernommen und geprüft", "Alle Zahlen stimmen und jede Datei ist in Ordnung."],
    WARN: ["WARN", "Übernommen, mit Hinweisen", "Bitte die Hinweise lesen. Oft war eine Datei schon in paperless-ngx defekt oder ein Vorschaubild fehlt – meist harmlos."],
    FAIL: ["FAIL", "Etwas stimmt nicht", "Deine alten Daten sind unverändert. Bitte den Bericht lesen oder weitergeben, bevor du mit PaperRust arbeitest."],
  };

  function showSwitch(sw) {
    const v = VERDICT[sw.verdict] ? sw.verdict : "FAIL";
    const [badge, title, hint] = VERDICT[v];
    const b = $("switch-badge");
    b.textContent = badge;
    b.className = "vbadge " + v.toLowerCase();
    $("switch-title").textContent = title;
    $("switch-hint").textContent = hint;
    show($("switch-spinner"), v === "RUNNING");
    const ul = $("switch-summary");
    ul.replaceChildren(...(sw.summary || []).map((t) => { const li = document.createElement("li"); li.textContent = t; return li; }));
    show($("switch-report"), !!sw.has_report);
    $("switch-open").textContent = v === "RUNNING" ? "Schon öffnen" : "PaperRust öffnen";
  }

  function mode() {
    if (formKind === "connect") return "remote";
    const r = form.querySelector("input[name=mode]:checked");
    return r ? r.value : "fresh";
  }

  function resetConfirm() {
    pendingPin = "";
    show($("http-confirm"), false);
    show($("tls-confirm"), false);
    show($("remote-info"), false);
    form.elements.allow_http.checked = false;
    form.elements.tls_ok.checked = false;
  }

  function submitLabel() {
    const m = mode();
    if (m === "remote") {
      if (!$("tls-confirm").hidden) return "Zertifikat vertrauen und verbinden";
      if (!$("http-confirm").hidden) return "Unverschlüsselt verbinden";
      return "Verbinden";
    }
    return m === "fresh" ? "Loslegen" : (m === "import" ? "Importieren" : "Übernehmen");
  }

  function updateFields() {
    const m = mode();
    show($("fields-fresh"), m === "fresh");
    show($("fields-import"), m === "import");
    show($("fields-migrate"), m === "migrate-db");
    show($("fields-remote"), m === "remote");
    $("submit").textContent = submitLabel();
    $("form-error").textContent = "";
  }

  const INTRO = {
    first: "Wie möchtest du PaperRust nutzen?",
    setup: "Wie soll diese PaperRust-Installation beginnen?",
    connect: "Adresse deines PaperRust- oder paperless-ngx-Servers. Die Anmeldung folgt danach im Fenster – auf der Anmeldeseite des Servers (auch mit Zwei-Faktor oder Single Sign-on).",
  };

  function openForm(kind, s) {
    if (formKind === kind && !form.hidden) return;
    formKind = kind;
    const first = kind === "first";
    show($("mode-choice"), kind === "setup" || first);
    show($("group-local"), first);
    show($("group-remote"), first);
    show($("choice-remote"), first);
    show($("choice-fresh"), kind !== "connect");
    show($("choice-import"), kind !== "connect");
    show($("choice-migrate"), (kind === "setup" || first) && s.can_migrate_db);
    $("setup-title").textContent = kind === "connect" ? "Mit einem Server verbinden" : "Willkommen";
    $("setup-intro").textContent = INTRO[kind];
    // Cancel: first start/remote without a profile = quit; from the settings = back
    $("cancel").textContent = kind === "connect" && s.connect_from_settings ? "Zurück" : "Abbrechen";
    resetConfirm();
    updateFields();
    show(form, true);
    const focus = kind === "connect" ? form.elements.server_url
      : form.querySelector("input[name=mode]:checked");
    if (focus) focus.focus();
  }

  function closeForm() {
    show(form, false);
    formKind = null;
  }

  function fmtTime(ms) {
    if (!ms) return null;
    try { return new Date(Number(ms)).toLocaleString("de-CH"); } catch { return null; }
  }

  function showOffline(o) {
    $("offline-profile").textContent = o.profile;
    $("offline-url").textContent = o.url;
    $("offline-error").textContent = o.error;
    const last = fmtTime(o.last_ok);
    $("offline-last").textContent = last ? "Zuletzt verbunden: " + last : "In dieser Sitzung noch nicht verbunden.";
    const tick = () => {
      const s = Math.max(0, Math.round((o.retry_at_ms - Date.now()) / 1000));
      $("offline-next").textContent = s > 0 ? "Neuer Versuch in " + s + " s …" : "Verbindung wird geprüft …";
    };
    tick();
    clearInterval(offlineTimer);
    offlineTimer = setInterval(tick, 1000);
  }

  function only(view) {
    for (const id of ["status-view", "error-view", "offline-view", "sso-view", "switch-view"]) show($(id), id === view);
    if (view !== "offline-view") clearInterval(offlineTimer);
  }

  async function poll() {
    if (restarting) return;
    let s;
    try {
      s = await invoke("shell_state");
    } catch (e) {
      $("stage").textContent = "Interner Fehler: " + e;
      return setTimeout(poll, 1000);
    }
    const remote = s.mode === "remote";
    if (s.sso) {
      closeForm();
      $("sso-origin").textContent = s.sso;
      only("sso-view");
      return setTimeout(poll, 500);
    }
    if (s.need_connect || s.need_first_start) {
      only(null);
      openForm(s.need_connect ? "connect" : "first", s);
      return setTimeout(poll, 500);
    }
    if (formKind === "connect" || formKind === "first") closeForm();
    if (s.error) {
      $("error-title").textContent = remote ? "Verbindung nicht möglich" : "PaperRust konnte nicht starten";
      $("error").textContent = s.error;
      $("advice").textContent = s.advice || "";
      show($("advice"), !!s.advice);
      show($("error-settings"), remote);
      errorLog = s.error_log;
      $("log-link").textContent = s.error_log ? "Log öffnen (" + s.error_log + ")" : "Log öffnen";
      closeForm();
      only("error-view");
      $("retry").disabled = false;
      return setTimeout(poll, 1000); // keep polling: "Erneut versuchen" resets the state
    }
    if (s.offline) {
      closeForm();
      showOffline(s.offline);
      only("offline-view");
      $("offline-retry").disabled = false;
      return setTimeout(poll, 1000);
    }
    if (s.ready_url && s.switch && !switchSeen) {
      // after "Von paperless-ngx umsteigen": first show the result of the reconciliation
      closeForm();
      showSwitch(s.switch);
      only("switch-view");
      readyUrl = s.ready_url;
      return setTimeout(poll, 700);
    }
    if (s.ready_url) {
      $("stage").textContent = remote ? "Verbunden – " + (s.profile || "Server") + " wird geöffnet …" : "Fertig – PaperRust wird geöffnet …";
      only("status-view");
      window.location.replace(s.ready_url);
      return;
    }
    const asking = s.need_setup;
    only(asking ? null : "status-view");
    if (s.need_setup) openForm("setup", s);
    else closeForm();
    $("stage").textContent = s.stage || "Wird gestartet …";
    setTimeout(poll, 300);
  }

  form.addEventListener("change", (ev) => {
    if (ev.target && ev.target.name === "mode") updateFields();
  });
  form.elements.server_url.addEventListener("input", () => { resetConfirm(); updateFields(); });

  async function connect() {
    const f = form.elements;
    const req = { url: f.server_url.value, name: f.profile_name.value, allow_http: false, tls_pin: "" };
    if (!$("http-confirm").hidden) {
      if (!f.allow_http.checked) { $("form-error").textContent = "Bitte die unverschlüsselte Verbindung bestätigen."; return; }
      req.allow_http = true;
    }
    if (!$("tls-confirm").hidden) {
      if (!f.tls_ok.checked) { $("form-error").textContent = "Bitte den Fingerabdruck vergleichen und bestätigen."; return; }
      req.tls_pin = pendingPin;
    }
    $("submit").textContent = "Server wird geprüft …";
    const r = await invoke("server_connect", { form: req });
    if (r.idn_host) {
      $("remote-info").textContent = "Internationaler Name – wird aufgerufen als " + r.idn_host + ".";
      show($("remote-info"), true);
    }
    if (r.status === "confirm_http") {
      $("http-message").textContent = r.message;
      show($("http-confirm"), true);
    } else if (r.status === "confirm_tls") {
      const c = r.cert;
      pendingPin = c.spki_sha256;
      $("tls-message").textContent = r.message;
      $("cert-subject").textContent = (c.subject_cn || "–") + (c.self_signed ? " (selbst signiert)" : "");
      $("cert-issuer").textContent = c.issuer_cn || "–";
      $("cert-until").textContent = c.not_after;
      $("cert-fp").textContent = c.fingerprint;
      f.tls_ok.checked = false;
      show($("tls-confirm"), true);
    } else if (r.status === "ok") {
      restarting = true;
      closeForm();
      only("status-view");
      $("stage").textContent = r.message + (r.note ? " – " + r.note : "") + ". PaperRust startet neu …";
      return;
    } else {
      $("form-error").textContent = r.message;
    }
    $("submit").textContent = submitLabel();
  }

  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    if (busy) return;
    const f = form.elements;
    const m = mode();
    busy = true;
    $("submit").disabled = true;
    $("form-error").textContent = "";
    try {
      if (m === "remote") {
        await connect();
        return;
      }
      const setup = {
        mode: m,
        source: m === "import" ? f.source_import.value : (m === "migrate-db" ? f.source_db.value : ""),
        media: m === "migrate-db" ? f.media.value : "",
      };
      if (formKind === "first") {
        await invoke("first_start_local", { form: setup });
      } else {
        await invoke("submit_setup", { form: setup });
      }
      closeForm();
      only("status-view");
    } catch (e) {
      $("form-error").textContent = String(e);
      $("submit").textContent = submitLabel();
    } finally {
      busy = false;
      $("submit").disabled = false;
    }
  });

  $("cancel").addEventListener("click", () => {
    if (formKind === "connect") invoke("cancel_connect");
    else invoke("cancel_admin");
  });
  $("quit").addEventListener("click", () => invoke("quit"));
  $("switch-report").addEventListener("click", () => invoke("open_log", { name: "umstieg-bericht.html" }));
  $("switch-open").addEventListener("click", () => {
    switchSeen = true;
    only("status-view");
    $("stage").textContent = "PaperRust wird geöffnet …";
    if (readyUrl) window.location.replace(readyUrl);
  });
  $("retry").addEventListener("click", async () => {
    $("retry").disabled = true;
    try { await invoke("retry"); } catch (e) { $("advice").textContent = String(e); show($("advice"), true); }
  });
  $("offline-retry").addEventListener("click", async () => {
    $("offline-retry").disabled = true;
    $("offline-next").textContent = "Verbindung wird geprüft …";
    await invoke("retry");
  });
  for (const id of ["offline-settings", "error-settings"]) $(id).addEventListener("click", () => invoke("open_settings"));
  $("sso-allow").addEventListener("click", () => invoke("sso_decide", { choice: "allow" }));
  $("sso-browser").addEventListener("click", () => invoke("sso_decide", { choice: "browser" }));
  $("sso-cancel").addEventListener("click", () => invoke("sso_decide", { choice: "cancel" }));
  $("log-link").addEventListener("click", (ev) => { ev.preventDefault(); invoke("open_log", { name: errorLog }); });
  $("logs-dir").addEventListener("click", (ev) => { ev.preventDefault(); invoke("open_log", { name: null }); });
  poll();
})();
