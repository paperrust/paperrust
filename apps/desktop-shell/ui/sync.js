// Settings → offline data (P5, docs/SYNC.md §4.4, docs/REMOTE-APP.md §2). Switching on/off
// via the command `sync_profile_set` (restarts), everything else via the client core under
// prapi://…/local/… (status, log in, check/adopt selection, limits, reconcile now,
// log out). This page never sees the token.
"use strict";
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const $ = (id) => document.getElementById(id);
  // Windows/WebView2: own schemes as http://<scheme>.localhost
  const PRAPI = /Windows/.test(navigator.userAgent) ? "http://prapi.localhost" : "prapi://localhost";
  const GIB = 1024 * 1024 * 1024;
  let profile = null;
  let local = null;
  let selection = [];

  function err(e) { $("settings-error").textContent = e ? String(e) : ""; }

  async function api(method, path, body) {
    const init = { method, headers: { Accept: "application/json" } };
    if (body !== undefined) {
      init.headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }
    const r = await fetch(PRAPI + path, init);
    const data = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error((data.error && data.error.message) || ("HTTP " + r.status));
    return data;
  }

  function size(b) {
    return b >= GIB ? (b / GIB).toFixed(1) + " GiB" : (b / 1048576).toFixed(1) + " MiB";
  }

  const STATES = {
    idle: "aktuell", syncing: "gleicht ab …", offline: "offline – lokaler Bestand",
    auth_required: "Anmeldung nötig", limit_exceeded: "Auswahl zu groß", pin_mismatch: "Zertifikat abgelehnt",
    error: "Fehler",
  };

  function renderStatus() {
    const s = local.status;
    let t = "Zustand: " + (STATES[s.state] || s.state) + " · " + local.documents + " Dokumente · Dateien " + size(local.cache_bytes);
    if (s.last_sync) t += " · zuletzt " + new Date(s.last_sync).toLocaleString();
    if (s.state === "syncing" && s.total) t += " · " + s.phase + " " + s.done + "/" + s.total;
    if (!s.token_persistent) t += " · Token nur bis zum Beenden (kein Schlüsselbund)";
    if (s.last_error && s.state !== "idle") t += " · " + s.last_error;
    $("sync-status").textContent = t;
    $("sync-login").hidden = s.logged_in;
    $("sync-selection").hidden = !s.logged_in;
  }

  async function renderItems() {
    const ul = $("sync-items");
    ul.textContent = "";
    const chosen = new Set(selection.map((i) => i.type + ":" + i.key));
    const [tags, views] = await Promise.all([
      api("GET", "/rs/api/v2/tags?limit=1000"),
      api("GET", "/rs/api/v2/saved_views?limit=1000"),
    ]);
    const add = (type, o) => {
      const li = document.createElement("li");
      const l = document.createElement("label");
      l.className = "check";
      const cb = document.createElement("input");
      cb.type = "checkbox";
      cb.dataset.type = type;
      cb.dataset.key = o.uuid;
      cb.checked = chosen.has(type + ":" + o.uuid);
      l.append(cb, " " + (type === "tag" ? "Tag: " : "Ansicht: ") + o.name);
      li.append(l);
      ul.append(li);
    };
    for (const t of tags.items || []) add("tag", t);
    for (const v of views.items || []) add("saved_view", v);
    if (selection.length) $("sync-files").value = selection[0].files;
  }

  function currentItems() {
    const files = $("sync-files").value;
    const items = [];
    for (const cb of document.querySelectorAll("#sync-items input[type=checkbox]")) {
      if (cb.checked) items.push({ type: cb.dataset.type, key: cb.dataset.key, files });
    }
    // individually selected documents (from the app) are kept
    for (const i of selection) if (i.type === "document") items.push(i);
    return items;
  }

  async function saveLimits() {
    const docs = parseInt($("sync-max-docs").value, 10);
    const gib = parseFloat($("sync-max-gib").value);
    if (docs > 0 && gib > 0) {
      await api("PUT", "/local/limits", { max_documents: docs, max_bytes: Math.round(gib * GIB) });
    }
  }

  function showPlan(p) {
    let t = p.total_documents + " Dokumente, " + size(p.total_bytes) + ".";
    if (!p.within_limits) t = (p.message || "Über der Grenze.") + " Bitte Auswahl verkleinern oder Grenze erhöhen.";
    $("sync-preview").textContent = t;
  }

  // full = redraw selection/limits (otherwise only the status – unsaved tick marks stay)
  let itemsShown = false;
  async function refresh(full) {
    local = await api("GET", "/local/status");
    renderStatus();
    if (full || !itemsShown) {
      selection = local.selection || [];
      $("sync-max-docs").value = local.limits.max_documents;
      $("sync-max-gib").value = (local.limits.max_bytes / GIB).toFixed(1);
      if (local.status.logged_in) { await renderItems(); itemsShown = true; }
    }
  }

  async function init() {
    try {
      profile = await invoke("sync_profile_state");
    } catch (e) { return; }
    if (!profile) return;
    $("sync-card").hidden = false;
    $("sync-enabled").checked = profile.sync;
    $("sync-enabled").disabled = !profile.supported;
    $("sync-unsupported").hidden = profile.supported;
    $("sync-enabled").addEventListener("change", async (ev) => {
      err(null);
      try {
        await invoke("sync_profile_set", { id: profile.id, sync: ev.target.checked });
        $("settings-error").textContent = "PaperRust startet neu …";
      } catch (e) { err(e); ev.target.checked = !ev.target.checked; }
    });
    // Card "Oberfläche (Fern-Modus)": app UI (2b) selectable as soon as PaperRust + light UI build
    const appRadio = document.querySelector("input[name=ui][value=app]");
    const srvRadio = document.querySelector("input[name=ui][value=server]");
    if (appRadio && srvRadio) {
      const usable = profile.supported && profile.app_available;
      appRadio.disabled = !usable;
      const label = appRadio.closest("label");
      if (label) {
        label.classList.toggle("disabled", !usable);
        const small = label.querySelector("small");
        if (small && usable) small.textContent = "Leichte Oberfläche aus der App; mit Offline-Bestand auch ohne Verbindung lesen und suchen.";
      }
      appRadio.checked = profile.ui === "app";
      srvRadio.checked = profile.ui !== "app";
      for (const r of [appRadio, srvRadio]) {
        r.addEventListener("change", async () => {
          if (!r.checked) return;
          err(null);
          try {
            await invoke("sync_profile_set_ui", { id: profile.id, ui: r.value });
            $("settings-error").textContent = "PaperRust startet neu …";
          } catch (e) { err(e); appRadio.checked = profile.ui === "app"; srvRadio.checked = profile.ui !== "app"; }
        });
      }
    }
    if (!profile.running) return;
    $("sync-body").hidden = false;

    $("sync-login").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      err(null);
      const pass = $("sync-pass");
      try {
        await api("POST", "/local/login", { username: $("sync-user").value, password: pass.value, code: $("sync-code").value || null });
        await refresh(true);
      } catch (e) { err(e); } finally { pass.value = ""; }
    });
    $("sync-check").addEventListener("click", async () => {
      err(null);
      try { await saveLimits(); showPlan(await api("POST", "/local/selection/preview", { items: currentItems() })); } catch (e) { err(e); }
    });
    $("sync-apply").addEventListener("click", async () => {
      err(null);
      try { await saveLimits(); showPlan(await api("PUT", "/local/selection", { items: currentItems() })); await refresh(true); } catch (e) { err(e); }
    });
    $("sync-now").addEventListener("click", async () => {
      err(null);
      try { await api("POST", "/local/sync"); setTimeout(refresh, 1500); } catch (e) { err(e); }
    });
    $("sync-logout").addEventListener("click", async () => {
      err(null);
      if (!confirm("Abmelden und den Offline-Bestand dieses Profils löschen?")) return;
      try { await api("POST", "/local/logout", { wipe: true }); await refresh(true); } catch (e) { err(e); }
    });
    try { await refresh(true); } catch (e) { err(e); }
    setInterval(() => { refresh(false).catch(() => {}); }, 10000);
  }

  init();
})();
