// Update window (local page, docs/DESKTOP.md §23): offer a ready, checked update with
// release notes; "Jetzt neu starten und installieren" or "Beim nächsten Start".
// Release notes are plain text (textContent), never HTML.
"use strict";
(function () {
  const invoke = window.__TAURI__.core.invoke;
  const $ = (id) => document.getElementById(id);
  let installing = false;

  function err(e) { $("error").textContent = e ? String(e) : ""; }

  function when(ms) {
    if (!ms) return "noch nie";
    return new Date(ms).toLocaleString();
  }

  function render(s) {
    $("current").textContent = "Installiert: PaperRust " + s.current + " · Kanal " + s.channel +
      (s.enabled ? " · zuletzt gesucht: " + when(s.last_check_ms) : "");
    $("note").hidden = !s.note;
    $("note").textContent = s.note || "";
    const p = s.enabled ? s.pending : null;
    $("ready").hidden = !p;
    $("idle").hidden = !!p;
    if (p) {
      $("title").textContent = "Update verfügbar";
      $("new-version").textContent = p.version;
      const mb = (p.size / 1048576).toFixed(1).replace(".", ",");
      $("ready-meta").textContent = "Kanal " + p.channel + (p.pub_date ? " · veröffentlicht " + p.pub_date.slice(0, 10) : "") +
        " · " + mb + " MB · Signatur geprüft";
      $("notes").textContent = p.notes || "Keine Versionshinweise.";
      $("not-installable").hidden = s.installable;
      $("install").disabled = installing || s.installing || !s.installable;
      $("install").textContent = installing || s.installing ? "Wird beendet …" : "Jetzt neu starten und installieren";
    } else {
      $("title").textContent = "Update";
      let t;
      if (!s.enabled) t = "Automatische Updates sind abgeschaltet: " + (s.reason || "");
      else if (s.checking) t = "Suche nach Updates …";
      else if (s.available) t = "Version " + s.available + " ist verfügbar. Diese Installation aktualisiert sich nicht selbst – bitte von Hand installieren.";
      else if (s.last_error) t = "Die letzte Suche bzw. Prüfung ist fehlgeschlagen:";
      else if (s.last_check_ms) t = "PaperRust ist aktuell.";
      else t = "Noch nicht nach Updates gesucht.";
      $("idle-text").textContent = t;
      $("check").disabled = !s.enabled || s.checking;
    }
    // Errors of the last search/check only without a ready update (otherwise outdated);
    // errors of a click are in #error.
    $("search-error").textContent = !p && s.last_error ? s.last_error : "";
  }

  async function load() {
    try { render(await invoke("update_state")); } catch (e) { err(e); }
  }

  $("install").addEventListener("click", async () => {
    err(null);
    installing = true;
    $("install").disabled = true;
    try {
      await invoke("update_install");
    } catch (e) {
      installing = false;
      err(e);
      load();
    }
  });
  $("later").addEventListener("click", () => invoke("update_later"));
  $("close").addEventListener("click", () => invoke("update_later"));
  $("check").addEventListener("click", async () => {
    err(null);
    try { await invoke("update_check"); } catch (e) { err(e); }
    load();
  });
  load();
  setInterval(load, 1000);
})();
