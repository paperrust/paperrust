// Browser E2E click paths (Playwright, Chromium) – once per side (A = Django, B = PaperRust).
//
//   node e2e.mjs --side A --base http://127.0.0.1:8170 --out <dir> [--flow full|branding|attr] [--upload 0|1]
//   (--flow attr --repeat N: only steps 36–41 N times, writing API calls with timing in mutations.json)
//
// Writes per step <out>/<nn>-<name>.png and <out>/steps.json (visible text, tables,
// network errors 4xx/5xx, console errors, error of the step). The comparison A↔B: compare.mjs.
// Every step is encapsulated on its own: if it fails, this is noted and the run continues
// (the following steps run anyway, so that A and B have the same step list).
import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith("--") ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const BASE = args.base;
const OUT = args.out;
const FLOW = args.flow || "full";
const UPLOAD = args.upload === "1";
const SIDE = args.side || "?";
fs.mkdirSync(OUT, { recursive: true });

const users = JSON.parse(fs.readFileSync(new URL("../../reference/users.json", import.meta.url), "utf8")).users;
const admin = users.find((u) => u.name === "admin");

// Masks: everything that depends on time or randomness (applies to A and B equally)
const MASK_CSS = [
  "pngx-toasts", ".toast-container", ".toast", // Toasts appear time-dependently (text is captured separately)
  ".popover", ".tooltip",
].join(",") + "{visibility:hidden !important}";
const MASK_SELECTORS = [
  "pngx-document-notes .card-footer small", // "x minutes ago" / creation time of new notes
  "pngx-profile-edit-dialog input[readonly]", // API token (random after regeneration)
  "pngx-profile-edit-dialog .font-monospace",
  "pngx-statistics-widget .text-muted.small", // nothing time-related, but placeholder if needed
  "pngx-upload-file-widget .card-body .progress",
];

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 1,
  locale: "en-US",
  timezoneId: "Europe/Zurich",
  reducedMotion: "reduce",
  colorScheme: "light",
});
const page = await context.newPage();
page.setDefaultTimeout(10000);

// No fixed browser clock: page.clock.setFixedTime freezes Date.now and thereby RxJS debounceTime
// (dirty check, text filter) – forms would never become "dirty". A and B run on the same day.

let netErrors = [];
let consoleErrors = [];
const allRequests = [];
page.on("response", (r) => {
  const req = r.request();
  const rec = { method: req.method(), url: r.url().replace(BASE, ""), status: r.status() };
  allRequests.push(rec);
  if (r.status() >= 400) netErrors.push(rec);
});
page.on("requestfailed", (req) => {
  const f = req.failure()?.errorText || "failed";
  if (f.includes("ERR_ABORTED")) return; // aborted navigations/prefetches are not an error
  netErrors.push({ method: req.method(), url: req.url().replace(BASE, ""), status: 0, error: f });
});
page.on("console", (m) => {
  if (m.type() === "error") consoleErrors.push(m.text().replaceAll(BASE, "").slice(0, 500));
});
page.on("pageerror", (e) => consoleErrors.push("pageerror: " + String(e.message).slice(0, 500)));

const steps = [];
let n = 0;

async function settle(ms = 600) {
  try { await page.waitForLoadState("networkidle", { timeout: 15000 }); } catch {}
  // wait for Angular spinner/"Loading..."
  try {
    await page.waitForFunction(() => ![...document.querySelectorAll(".spinner-border, .placeholder-glow")].some((e) => e.offsetWidth || e.offsetHeight), null, { timeout: 8000 });
  } catch {}
  await page.waitForTimeout(ms);
}

async function capture() {
  return page.evaluate(() => {
    const vis = (el) => !!(el.offsetWidth || el.offsetHeight || el.getClientRects().length);
    const toasts = [...document.querySelectorAll(".toast")].filter(vis).map((t) => t.innerText.trim().replace(/\s+/g, " "));
    const tables = [...document.querySelectorAll("table")].filter(vis).map((t) =>
      [...t.querySelectorAll("tr")].filter(vis).map((tr) => [...tr.querySelectorAll("th,td")].map((c) => c.innerText.trim().replace(/\s+/g, " "))),
    );
    const text = (document.body?.innerText || "").split("\n").map((l) => l.trim()).filter(Boolean);
    return { url: location.pathname + location.search, title: document.title, text, tables, toasts };
  });
}

// Server reachable before every step? If it is gone (e.g. crash of the granian worker in A), the
// suite waits up to 180 s for the restart by the watchdog in run.sh and notes this in the step (serverDown).
const serverDowns = [];
async function waitServer() {
  const t0 = Date.now();
  let down = false;
  while (Date.now() - t0 < 180000) {
    try {
      const r = await fetch(BASE + "/api/", { redirect: "manual", signal: AbortSignal.timeout(5000) });
      if (r.status > 0) break;
    } catch { down = true; }
    await new Promise((r) => setTimeout(r, 2000));
  }
  return down ? Math.round((Date.now() - t0) / 1000) : 0;
}

async function step(name, fn, opts = {}) {
  n += 1;
  const id = String(n).padStart(2, "0") + "-" + name;
  const downSecs = await waitServer();
  if (downSecs) {
    serverDowns.push(id);
    console.log(`[${SIDE}] Server ${BASE} war vor ${id} nicht erreichbar, nach ${downSecs} s wieder da`);
    if (opts.reloadAfterDown !== false) { try { await page.reload({ waitUntil: "domcontentloaded" }); await settle(); } catch {} }
  }
  netErrors = [];
  consoleErrors = [];
  const t0 = Date.now();
  const reqMark = allRequests.length;
  let error = null;
  let skipped = null;
  try {
    const r = await fn();
    if (r && r.skip) skipped = r.skip;
    await settle(opts.wait ?? 600);
  } catch (e) {
    error = String(e.message || e).split("\n")[0].slice(0, 300);
    await settle(200);
  }
  let snap = { url: page.url(), text: [], tables: [], toasts: [] };
  try { snap = await capture(); } catch (e) { error = error || "capture: " + e.message; }
  const file = id + ".png";
  try {
    await page.screenshot({
      path: path.join(OUT, file),
      animations: "disabled",
      caret: "hide",
      style: MASK_CSS,
      mask: [...MASK_SELECTORS, ...(opts.mask || [])].map((s) => page.locator(s)),
      maskColor: "#ff00ff",
    });
  } catch (e) { error = error || "screenshot: " + e.message; }
  const api = allRequests.slice(reqMark).filter((r) => r.url.startsWith("/api/")).map((r) => `${r.method} ${r.url} ${r.status}`);
  steps.push({ id, name, ms: Date.now() - t0, error, skipped, serverDownBefore: downSecs || 0, ...snap, api, netErrors, consoleErrors: [...consoleErrors] });
  console.log(`[${SIDE}] ${id} ${error ? "FEHLER " + error : skipped ? "übersprungen: " + skipped : "ok"} (${Date.now() - t0} ms)`);
}

// ---------- Helpers for the Angular UI (paperless-ngx v3.4.2) ----------
const sidebar = (label) => page.locator("pngx-app-frame nav").getByRole("link", { name: label, exact: true }).first();
const navTab = (label) => page.locator("pngx-document-detail a.nav-link", { hasText: label }).first();
const modal = () => page.locator(".modal.show .modal-content").last();

async function gotoUi(p) {
  await page.goto(BASE + p, { waitUntil: "domcontentloaded" });
  await page.locator("pngx-app-frame").waitFor({ timeout: 30000 });
}

async function login(user, pass) {
  await page.goto(BASE + "/accounts/login/", { waitUntil: "domcontentloaded" });
  await page.fill("#inputUsername", user);
  await page.fill("#inputPassword", pass);
  await Promise.all([page.waitForLoadState("domcontentloaded"), page.click("button[type=submit]")]);
}

async function filterDropdown(title, itemText) {
  const dd = page.locator("pngx-filter-editor pngx-filterable-dropdown", { has: page.locator(`button[aria-label="${title}"]`) }).first();
  await dd.locator(`button[aria-label="${title}"]`).click();
  const menu = dd.locator(".dropdown-menu");
  await menu.locator("input.form-control").fill(itemText);
  await menu.locator("button.list-group-item, .list-group-item", { hasText: itemText }).first().click();
  await page.keyboard.press("Escape");
}

async function attrCrud(section, singularBtn, name, renamed) {
  // create
  await gotoUi("/attributes/" + section);
  await settle();
  await page.getByRole("button", { name: "Create", exact: true }).click();
  const m = modal();
  await m.locator('pngx-input-text[formcontrolname="name"] input').fill(name);
  const color = m.locator('pngx-input-color input[type="text"], pngx-input-color input.form-control').first();
  if (await color.count()) await color.fill("#336699");
  await m.getByRole("button", { name: "Save", exact: true }).click();
  await m.waitFor({ state: "detached", timeout: 15000 }).catch(() => {});
}

async function attrRename(section, name, renamed) {
  await gotoUi("/attributes/" + section);
  await settle();
  const row = page.locator("tr", { hasText: name }).first();
  await row.getByRole("button", { name: "Edit" }).click();
  const m = modal();
  await m.locator('pngx-input-text[formcontrolname="name"] input').fill(renamed);
  await m.getByRole("button", { name: "Save", exact: true }).click();
  await m.waitFor({ state: "detached", timeout: 15000 }).catch(() => {});
}

async function attrDelete(section, name) {
  await gotoUi("/attributes/" + section);
  await settle();
  const row = page.locator("tr", { hasText: name }).first();
  await row.getByRole("button", { name: "Delete" }).click();
  const m = modal();
  await m.locator(".modal-footer button.btn-danger").click({ timeout: 20000 });
  await m.waitFor({ state: "detached", timeout: 15000 }).catch(() => {});
}

// Writing API calls with browser timing (repeat runs, --flow attr): when sent,
// when on the wire (requestStart), when response head (responseStart), result.
const mutations = [];
function trackMutation(req, outcome) {
  if (!["POST", "PUT", "PATCH", "DELETE"].includes(req.method())) return;
  const url = req.url().replace(BASE, "");
  if (!url.startsWith("/api/")) return;
  const t = req.timing();
  mutations.push({
    at: new Date(t.startTime).toISOString(), method: req.method(), url, outcome,
    connect_ms: t.connectEnd >= 0 ? Math.round(t.connectEnd - Math.max(t.connectStart, 0)) : null,
    queued_ms: t.requestStart >= 0 ? Math.round(t.requestStart) : null,
    head_ms: t.responseStart >= 0 ? Math.round(t.responseStart) : null,
    total_ms: t.responseEnd >= 0 ? Math.round(t.responseEnd) : null,
  });
}
page.on("requestfinished", async (req) => { const r = await req.response().catch(() => null); trackMutation(req, String(r?.status() ?? "?")); });
page.on("requestfailed", (req) => trackMutation(req, "failed: " + (req.failure()?.errorText || "?")));

// ---------------------------------------------------------------------------
if (FLOW === "attr") {
  // Repeat run of steps 36–41 (create/rename/delete document type, settings),
  // --repeat N. B diagnosis only (hang at the proxy, E2E 2026-11-08); no comparison with A.
  await step("login", async () => {
    // after a reset Django is cold (first pages > 10 s): up to 3 attempts
    for (let a = 1; ; a++) {
      try { await login(admin.username, admin.password); await page.locator("pngx-dashboard").waitFor({ timeout: 30000 }); return; }
      catch (e) { if (a >= 3) throw e; }
    }
  });
  const N = Number(args.repeat || 10);
  for (let i = 1; i <= N; i++) {
    const name = `E2E dokumenttyp ${i}`;
    await step(`r${i}-dokumenttyp-liste`, async () => { await gotoUi("/attributes/documenttypes"); });
    await step(`r${i}-dokumenttyp-anlegen`, async () => {
      await attrCrud("documenttypes", "dokumenttyp", name);
      if (await modal().count()) throw new Error("Dialog nach Save noch offen (Antwort fehlt)");
    });
    await step(`r${i}-dokumenttyp-umbenennen`, async () => {
      await attrRename("documenttypes", name, name + " neu");
      if (await modal().count()) throw new Error("Dialog nach Save noch offen (Antwort fehlt)");
    });
    await step(`r${i}-dokumenttyp-loeschen`, async () => attrDelete("documenttypes", name + " neu"));
    await step(`r${i}-einstellungen`, async () => { await gotoUi("/settings"); await page.locator("pngx-settings").waitFor(); });
  }
} else if (FLOW === "branding") {
  await step("login-seite", async () => { await page.goto(BASE + "/accounts/login/"); });
  await step("dashboard", async () => { await login(admin.username, admin.password); await page.locator("pngx-dashboard").waitFor(); });
  await step("dokumentliste", async () => { await gotoUi("/documents?sort=created&reverse=1"); await page.locator("pngx-document-list").waitFor(); });
  await step("dokument", async () => { await gotoUi("/documents/1/details"); await page.locator("pngx-document-detail").waitFor(); });
  await step("einstellungen", async () => { await gotoUi("/settings"); await page.locator("pngx-settings").waitFor(); });
} else {
  // 1. Login / Logout
  await step("login-seite", async () => { await page.goto(BASE + "/accounts/login/"); });
  await step("login-falsches-passwort", async () => { await login(admin.username, "falsch"); });
  await step("login-richtig-dashboard", async () => {
    await login(admin.username, admin.password);
    await page.locator("pngx-dashboard").waitFor({ timeout: 30000 });
    await page.locator("pngx-statistics-widget").first().waitFor({ timeout: 20000 });
  }, { wait: 1500 });
  await step("logout", async () => {
    await page.locator("#userDropdown").click();
    await page.getByRole("menuitem", { name: "Logout" }).or(page.getByRole("button", { name: "Logout" })).or(page.getByText("Logout", { exact: true })).first().click();
    await page.waitForURL(/accounts\/login|logout/, { timeout: 20000 });
  });
  await step("login-erneut", async () => {
    await login(admin.username, admin.password);
    await page.locator("pngx-dashboard").waitFor({ timeout: 30000 });
  }, { wait: 1500 });

  // 2. Document list
  await step("dokumentliste", async () => {
    await sidebar("Documents").click();
    await page.locator("pngx-document-list").waitFor();
    await page.locator("pngx-document-list table tbody tr, pngx-document-card-small").first().waitFor({ timeout: 20000 });
  }, { wait: 2500 }); // Owner column fills only after /api/users/
  await step("liste-tabellenansicht", async () => {
    await page.locator('label[for="displayModeDetails"]').click();
    await page.locator("pngx-document-list table tbody tr").first().waitFor({ timeout: 20000 });
  }, { wait: 2500 });
  await step("sortieren-titel", async () => {
    await page.getByRole("button", { name: "Sort" }).click();
    await page.locator(".dropdown-menu.show").getByRole("menuitem", { name: "Title", exact: true })
      .or(page.locator(".dropdown-menu.show button", { hasText: /^\s*Title\s*$/ })).first().click();
    await page.keyboard.press("Escape");
  });
  await step("sortieren-umkehren", async () => {
    await page.getByRole("button", { name: "Sort" }).click();
    await page.locator('label[for="listSortReverseTrue"]').click();
    await page.keyboard.press("Escape");
  });
  await step("seite-2", async () => {
    await page.locator("ngb-pagination").first().getByRole("link", { name: "2", exact: true }).click();
  });
  await step("filter-tag", async () => {
    await page.locator("ngb-pagination").first().getByRole("link", { name: "1", exact: true }).click().catch(() => {});
    await filterDropdown("Tags", "Invoices");
  });
  await step("filter-korrespondent", async () => { await filterDropdown("Correspondent", "ACME Corp"); });
  await step("filter-zuruecksetzen", async () => { await page.getByRole("button", { name: "Reset filters" }).first().click(); });
  await step("filter-text", async () => {
    const input = page.locator("pngx-filter-editor input.form-control[type=text]").first();
    await input.fill("Invoice");
    await page.waitForTimeout(800); // Debounce
  });
  await step("massenauswahl-seite", async () => {
    await page.getByRole("button", { name: "Page" }).first().click();
  });
  await step("massenauswahl-alle", async () => {
    await page.getByRole("button", { name: "All" }).first().click();
  });
  await step("massenauswahl-keine", async () => {
    await page.getByRole("button", { name: "None" }).first().click();
    await page.getByRole("button", { name: "Reset filters" }).first().click().catch(() => {});
  });

  // 3. Saved view
  await step("ansicht-speichern-als", async () => {
    await filterDropdown("Tags", "Unpaid");
    await page.getByRole("button", { name: "Views" }).click();
    await page.getByRole("menuitem", { name: "Save as..." }).or(page.getByText("Save as...", { exact: true })).first().click();
    const m = modal();
    await m.locator("input").first().fill("E2E Ansicht");
    await m.getByRole("button", { name: "Save", exact: true }).click();
    await m.waitFor({ state: "detached", timeout: 15000 }).catch(() => {});
  }, { wait: 1200 });

  await step("ansichten-liste", async () => { await gotoUi("/savedviews"); await page.locator("pngx-saved-views").waitFor(); });
  // 4. Open document
  await step("dokument-details", async () => {
    await gotoUi("/documents/1/details");
    await page.locator("pngx-document-detail").waitFor();
    await page.locator('pngx-input-text[formcontrolname="title"] input').waitFor({ timeout: 20000 });
  }, { wait: 2500 });
  await step("dokument-vorschau", async () => {
    // Preview on the right (pdf.js / img); wait until something is rendered
    await page.locator("pngx-pdf-viewer canvas, pngx-document-detail .preview-sticky img, pngx-document-detail object, pngx-document-detail iframe").first().waitFor({ timeout: 20000 });
  }, { wait: 2500 });
  await step("dokument-inhalt", async () => { await navTab("Content").click(); });
  await step("dokument-metadaten", async () => { await navTab("Metadata").click(); }, { wait: 1500 });
  await step("dokument-notizen", async () => { await navTab("Notes").click(); });
  await step("notiz-anlegen", async () => {
    await page.locator("pngx-document-notes textarea").fill("E2E-Notiz aus dem Browser");
    await page.getByRole("button", { name: "Add note" }).click();
    await page.locator("pngx-document-notes .card", { hasText: "E2E-Notiz aus dem Browser" }).first().waitFor({ timeout: 15000 });
  });
  await step("notiz-loeschen", async () => {
    const card = page.locator("pngx-document-notes .card", { hasText: "E2E-Notiz aus dem Browser" }).first();
    await card.locator('button[title="Delete note"]').click({ force: true });
    await card.waitFor({ state: "detached", timeout: 15000 });
  });
  await step("details-aendern-speichern", async () => {
    await navTab("Details").click();
    const t = page.locator('pngx-input-text[formcontrolname="title"] input');
    await t.fill("E2E geänderter Titel");
    await t.press("End"); await t.type(" "); await t.press("Backspace"); await t.press("Tab"); // real key events (dirty check)
    await page.locator('pngx-document-detail button[type="submit"][title="Save"]:not([disabled])').first().click({ timeout: 15000 });
    await page.waitForTimeout(1500);
  }, { wait: 1500 });
  await step("dokument-wieder-oeffnen", async () => {
    await gotoUi("/documents/1/details");
    await page.locator('pngx-input-text[formcontrolname="title"] input').waitFor({ timeout: 20000 });
  }, { wait: 2500 });

  // 5. Tags / correspondents / document types
  for (const [section, label] of [["tags", "tag"], ["correspondents", "korrespondent"], ["documenttypes", "dokumenttyp"]]) {
    const name = `E2E ${label}`;
    await step(`${label}-liste`, async () => { await gotoUi("/attributes/" + section); });
    await step(`${label}-anlegen`, async () => attrCrud(section, label, name));
    await step(`${label}-umbenennen`, async () => attrRename(section, name, name + " neu"));
    await step(`${label}-loeschen`, async () => attrDelete(section, name + " neu"));
  }

  // 6. Settings
  await step("einstellungen", async () => { await gotoUi("/settings"); await page.locator("pngx-settings").waitFor(); });
  await step("einstellungen-speichern", async () => {
    await page.locator("pngx-settings a.nav-link", { hasText: "Documents" }).first().click();
    const sel = page.locator("#documentListItemPerPage");
    if (await sel.count()) await sel.selectOption({ label: "25" });
    else await page.locator("pngx-settings input[type=checkbox]").first().click();
    await page.locator("pngx-settings").getByRole("button", { name: "Save", exact: true }).last().click();
  }, { wait: 1500 });

  // 8. Trash
  await step("papierkorb", async () => { await gotoUi("/trash"); await page.locator("pngx-trash").waitFor(); });
  await step("papierkorb-wiederherstellen", async () => {
    const row = page.locator("pngx-trash table tbody tr").first();
    await row.getByRole("button", { name: "Restore" }).click();
  }, { wait: 1500 });

  // 9. Profile / token
  await step("profil", async () => {
    await page.locator("#userDropdown").click();
    await page.getByText("My Profile", { exact: true }).first().click();
    await page.locator("pngx-profile-edit-dialog").waitFor();
  }, { wait: 1200 });
  await step("token-neu", async () => {
    const d = page.locator("pngx-profile-edit-dialog");
    await d.locator("pngx-confirm-button button").first().click();
    await page.locator(".popover").getByRole("button", { name: "Yes" }).click();
  }, { wait: 1500 });
  await step("profil-schliessen", async () => {
    await page.locator("pngx-profile-edit-dialog").getByRole("button", { name: "Cancel" }).click();
  });

  // 10. Dashboard at the end (shows new view etc.)
  await step("dashboard-ende", async () => { await gotoUi("/dashboard"); }, { wait: 1500 });
  // 11. Upload (last, so that the new record does not change the other steps)
  await step("upload", async () => {
    if (!UPLOAD) return { skip: "kein Worker auf dieser Seite (Django ohne Celery-Worker)" };
    await gotoUi("/dashboard");
    const pdf = fileURLToPath(new URL("./fixtures/e2e-upload.pdf", import.meta.url));
    await page.locator('input[type="file"]').first().setInputFiles(pdf);
    // wait for finished processing (upload status message via /ws/status/)
    await page.waitForTimeout(20000);
  }, { wait: 1500 });
  await step("upload-ergebnis", async () => {
    if (!UPLOAD) return { skip: "kein Upload (siehe Schritt upload)" };
    await gotoUi("/documents?sort=added&reverse=1");
    await page.locator("pngx-document-list").waitFor();
  }, { wait: 1500 });

}

if (mutations.length) fs.writeFileSync(path.join(OUT, "mutations.json"), JSON.stringify(mutations, null, 1));
fs.writeFileSync(path.join(OUT, "steps.json"), JSON.stringify({ side: SIDE, base: BASE, flow: FLOW, serverDowns, steps }, null, 1));
fs.writeFileSync(path.join(OUT, "requests.json"), JSON.stringify(allRequests, null, 0));
await browser.close();
