// Settings (local page): mode, server profiles (use, rename, log out,
// remove, add), UI. Switching restarts the app; local data stays untouched.
"use strict";
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const $ = (id) => document.getElementById(id);
  let state = null;

  function err(e) { $("settings-error").textContent = e ? String(e) : ""; }

  function button(text, cls, onclick) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = text;
    if (cls) b.className = cls;
    b.addEventListener("click", async () => {
      err(null);
      try { await onclick(); } catch (e) { err(e); }
    });
    return b;
  }

  function badge(text, cls) {
    const s = document.createElement("span");
    s.className = "badge " + (cls || "");
    s.textContent = text;
    return s;
  }

  function restarting(msg) {
    document.body.classList.add("busy");
    err(null);
    $("settings-error").textContent = msg || "PaperRust startet neu …";
  }

  function render() {
    const s = state;
    for (const r of document.querySelectorAll("input[name=mode]")) r.checked = r.value === s.mode;
    $("apply-mode").disabled = true;
    $("local-data").textContent = s.local_data
      ? "Lokale Daten (Datenbank, Dokumente) bleiben bei jedem Wechsel erhalten."
      : "Noch keine lokalen Daten auf diesem Computer.";
    $("home").textContent = s.home;
    const ul = $("profiles");
    ul.textContent = "";
    $("no-profiles").hidden = s.profiles.length > 0;
    for (const p of s.profiles) {
      const li = document.createElement("li");
      li.dataset.id = p.id;
      const head = document.createElement("div");
      head.className = "profile-head";
      const name = document.createElement("strong");
      name.textContent = p.name;
      head.append(name);
      if (p.active && s.mode === "remote") head.append(badge("aktiv", "ok"));
      head.append(badge(p.flavor));
      if (p.allow_http) head.append(badge("⚠ http unverschlüsselt", "warn"));
      if (p.tls_pinned) head.append(badge("eigenes Zertifikat bestätigt"));
      const url = document.createElement("div");
      url.className = "hint";
      url.textContent = p.url + (p.extra_origins.length ? " · Anmeldung auch über " + p.extra_origins.join(", ") : "");
      const actions = document.createElement("div");
      actions.className = "buttons";
      if (!(p.active && s.running === "remote")) {
        actions.append(button("Verwenden", "", async () => {
          await invoke("profile_activate", { id: p.id });
          restarting("Wechsel zu " + p.name + " – PaperRust startet neu …");
        }));
      }
      actions.append(button("Umbenennen", "secondary", async () => {
        const n = window.prompt("Neuer Name für " + p.name, p.name);
        if (n === null) return;
        await invoke("profile_rename", { id: p.id, name: n });
        await load();
      }));
      actions.append(button("Abmelden", "secondary", async () => {
        if (!window.confirm("Bei " + p.name + " abmelden? Die gespeicherte Anmeldung (Cookies) dieses Profils wird gelöscht.")) return;
        await invoke("profile_logout", { id: p.id });
        if (p.active && s.running === "remote") restarting("Abmelden – PaperRust startet neu …");
        else await load();
      }));
      actions.append(button("Entfernen", "secondary", async () => {
        if (!window.confirm("Profil " + p.name + " entfernen? Auf dem Server wird nichts gelöscht.")) return;
        await invoke("profile_remove", { id: p.id });
        if (p.active && s.running === "remote") restarting();
        else await load();
      }));
      li.append(head, url, actions);
      ul.append(li);
    }
  }

  async function load() {
    try {
      state = await invoke("settings_state");
      render();
    } catch (e) { err(e); }
    loadUpdates();
  }

  // ---- Updates (docs/DESKTOP.md §23)
  let upd = null;
  function updDirty() {
    if (!upd) return false;
    const ch = document.querySelector("input[name=channel]:checked");
    return $("upd-enabled").checked !== upd.settings_enabled ||
      (ch && ch.value !== upd.channel) ||
      Number($("upd-interval").value) !== upd.interval_hours;
  }
  function renderUpdates(s, fields) {
    upd = s;
    if (fields) {
      $("upd-enabled").checked = s.settings_enabled;
      for (const r of document.querySelectorAll("input[name=channel]")) {
        r.checked = r.value === s.channel;
        r.disabled = s.channel_from_env;
      }
      const sel = $("upd-interval");
      if (![...sel.options].some((o) => Number(o.value) === s.interval_hours)) {
        const o = document.createElement("option");
        o.value = String(s.interval_hours);
        o.textContent = s.interval_hours + " Stunden";
        sel.append(o);
      }
      sel.value = String(s.interval_hours);
    }
    let t = "Installiert: " + s.current + ". ";
    if (!s.enabled) t += "Abgeschaltet: " + (s.reason || "") + ".";
    else if (s.checking) t += "Suche läuft …";
    else if (s.pending) t += "Version " + s.pending.version + " ist bereit (Update-Fenster).";
    else if (s.last_error) t += "Letzte Suche fehlgeschlagen: " + s.last_error;
    else if (s.last_check_ms) t += "Aktuell (zuletzt gesucht " + new Date(s.last_check_ms).toLocaleString() + ").";
    if (s.channel_from_env) t += " Kanal durch Umgebung festgelegt.";
    if (s.endpoint_from_env) t += " Update-Adresse durch Umgebung: " + s.endpoint;
    $("upd-status").textContent = t;
    $("upd-save").disabled = !updDirty();
  }
  async function loadUpdates(fields = true) {
    try { renderUpdates(await invoke("update_state"), fields); } catch (e) { err(e); }
  }
  for (const id of ["upd-enabled", "upd-interval"]) $(id).addEventListener("change", () => { $("upd-save").disabled = !updDirty(); });
  document.addEventListener("change", (ev) => {
    if (ev.target && ev.target.name === "channel") $("upd-save").disabled = !updDirty();
  });
  $("upd-save").addEventListener("click", async () => {
    err(null);
    const ch = document.querySelector("input[name=channel]:checked");
    try {
      await invoke("update_settings_save", {
        enabled: $("upd-enabled").checked,
        channel: ch ? ch.value : "stable",
        intervalHours: Number($("upd-interval").value),
      });
      await loadUpdates(true);
    } catch (e) { err(e); }
  });
  $("upd-open").addEventListener("click", () => invoke("open_update"));
  setInterval(() => { if (!updDirty()) loadUpdates(false); }, 2000);

  document.addEventListener("change", (ev) => {
    if (ev.target && ev.target.name === "mode") {
      const chosen = document.querySelector("input[name=mode]:checked");
      $("apply-mode").disabled = !chosen || chosen.value === state.mode;
    }
  });
  $("apply-mode").addEventListener("click", async () => {
    const chosen = document.querySelector("input[name=mode]:checked");
    try {
      await invoke("settings_set_mode", { mode: chosen.value });
      restarting();
    } catch (e) { err(e); }
  });
  $("add-profile").addEventListener("click", () => invoke("begin_connect"));
  $("back").addEventListener("click", () => invoke("close_settings"));
  $("logs").addEventListener("click", (ev) => { ev.preventDefault(); invoke("open_log", { name: null }); });
  load();
})();
