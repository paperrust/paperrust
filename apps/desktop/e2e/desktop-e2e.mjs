// PaperRust Desktop – end-to-end test of the finished package (Tauri window via WebView2 CDP).
//
//   node apps/desktop/e2e/desktop-e2e.mjs <install folder> <data folder> <screenshot folder>
//
// Flow: start the app (fresh PAPERRUST_DESKTOP_HOME, own WebView2 profile), fill in the first-start form,
// log in, upload a scanned PDF (OCR with Tesseract + PDFium from the package), check content and
// full-text search, backup during operation (`paperrust-desktop backup`), quit
// (`paperrust-desktop stop`), restart, data still there, quit. Afterwards error page (database does not
// start) with its own data folder. Log: <screenshot folder>/e2e-log.json.
//
// Playwright comes from tests/e2e/node_modules (no new dependency). Never port 54329/8100/8101:
// the app chooses its ports itself (default 54417/58080, otherwise free).
import { createRequire } from "node:module";
import { spawn, spawnSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..", "..");
const require = createRequire(path.join(repo, "tests", "e2e", "package.json"));
const { chromium } = require("playwright");

const [installDir, homeDir, outDir] = process.argv.slice(2).map((p) => p && path.resolve(p));
if (!installDir || !homeDir || !outDir) {
  console.error("usage: node desktop-e2e.mjs <install> <home> <out>");
  process.exit(2);
}
fs.mkdirSync(outDir, { recursive: true });
const scanPdf = path.join(repo, "results", "e2e", "desktop", "scan-zahnarzt.pdf");
const ADMIN = { user: "admin", pass: "e2e-desktop-" + Date.now().toString(36) };
const log = [];
const t0 = Date.now();
function step(name, data = {}) {
  const e = { t: ((Date.now() - t0) / 1000).toFixed(1), step: name, ...data };
  log.push(e);
  console.log(`[${e.t}s] ${name}`, Object.keys(data).length ? JSON.stringify(data) : "");
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function saveLog(ok) {
  fs.writeFileSync(path.join(outDir, "e2e-log.json"), JSON.stringify({ ok, installDir, homeDir, steps: log }, null, 2));
}

function appEnv(home, cdpPort) {
  const env = { ...process.env };
  for (const k of Object.keys(env)) if (/^(PAPERRUST_|PAPERLESS_|PG)/.test(k)) delete env[k];
  env.PAPERRUST_DESKTOP_HOME = home;
  env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${cdpPort}`;
  env.WEBVIEW2_USER_DATA_FOLDER = path.join(path.dirname(home), path.basename(home) + "-webview2");
  // Tests do not query an update server (E2E_UPDATES=on switches the search back on)
  env.PAPERRUST_DESKTOP_UPDATES = process.env.E2E_UPDATES || "off";
  return env;
}

function startApp(home, cdpPort) {
  // E2E_DESKTOP_TOML: prepared desktop.toml (own ports) for a fresh data folder
  const toml = process.env.E2E_DESKTOP_TOML;
  if (toml && !fs.existsSync(path.join(home, "desktop.toml"))) {
    fs.mkdirSync(home, { recursive: true });
    fs.copyFileSync(toml, path.join(home, "desktop.toml"));
  }
  const exe = path.join(installDir, "paperrust-app.exe");
  const child = spawn(exe, [], { env: appEnv(home, cdpPort), stdio: "ignore", detached: false });
  step("app gestartet", { pid: child.pid, cdpPort });
  return child;
}

function desktopCli(home, args, timeoutMs = 600000) {
  const exe = path.join(installDir, "paperrust-desktop.exe");
  const start = Date.now();
  // Messages from paperrust-desktop go to stderr
  const r = spawnSync(exe, args, { env: appEnv(home, 0), timeout: timeoutMs, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  if (r.error) throw r.error;
  const out = `${r.stdout || ""}${r.stderr || ""}`.trim();
  if (r.status !== 0) throw new Error(`paperrust-desktop ${args.join(" ")}: Exit ${r.status}: ${out}`);
  return { out, ms: Date.now() - start };
}

async function connect(cdpPort) {
  for (let i = 0; i < 120; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${cdpPort}/json/version`);
      if (r.ok) break;
    } catch {}
    await sleep(500);
  }
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  for (let i = 0; i < 60; i++) {
    const pages = browser.contexts().flatMap((c) => c.pages());
    if (pages.length) return { browser, page: pages[0] };
    await sleep(500);
  }
  throw new Error("kein WebView-Fenster gefunden");
}

async function shot(page, name) {
  const f = path.join(outDir, name + ".png");
  await page.screenshot({ path: f });
  step("screenshot", { file: path.relative(repo, f) });
}

function processesFor(home) {
  // Processes whose command line names the data folder (postgres, pg_ctl) or that run from the
  // installation folder.
  const ps = `Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and ($_.CommandLine.Contains('${home.replace(/'/g, "''")}') -or $_.ExecutablePath -like '${installDir.replace(/'/g, "''")}*') } | ForEach-Object { "$($_.ProcessId) $($_.Name)" }`;
  const out = execFileSync("powershell.exe", ["-NoProfile", "-Command", ps], { encoding: "utf8" });
  return out.split(/\r?\n/).filter(Boolean);
}

async function waitExit(child, ms) {
  if (child.exitCode !== null) return child.exitCode;
  return await Promise.race([new Promise((r) => child.once("exit", (c) => r(c))), sleep(ms).then(() => "timeout")]);
}

async function api(page, method, url, body) {
  return await page.evaluate(async ({ method, url, body }) => {
    const csrf = (document.cookie.match(/(?:^|; )csrftoken=([^;]+)/) || [])[1] || "";
    const r = await fetch(url, { method, headers: { "X-CSRFToken": csrf, Accept: "application/json" }, body, credentials: "same-origin" });
    const text = await r.text();
    let json = null;
    try { json = JSON.parse(text); } catch {}
    return { status: r.status, json, text: json ? undefined : text.slice(0, 300) };
  }, { method, url, body });
}

async function main() {
  const cdp1 = 9300 + Math.floor(Math.random() * 300);
  // ---------------------------------------------------------------- 1. first start
  let app = startApp(homeDir, cdp1);
  let { browser, page } = await connect(cdp1);
  step("CDP verbunden", { url: page.url() });
  await page.waitForSelector("#setup-form:not([hidden])", { timeout: 180000 });
  step("Erst-Start-Formular sichtbar");
  await shot(page, "01-erststart-formular");
  await page.check("input[name=mode][value=import]");
  await shot(page, "02-erststart-import-option");
  await page.check("input[name=mode][value=fresh]");
  await shot(page, "03-erststart-ausgewaehlt");
  await page.click("#submit");
  step("Formular abgeschickt");
  // ---------------------------------------------------------------- 2. automatic login
  // No user name/password any more: the supervisor creates the local owner, the shell
  // opens /rs/desktop/login?token=… and lands logged in in the UI.
  await page.waitForURL(/http:\/\/127\.0\.0\.1:\d+\/(dashboard|rs\/app\/)/, { timeout: 300000 });
  const base = new URL(page.url()).origin;
  step("automatisch angemeldet", { base });
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(1500);
  step("angemeldet", { url: page.url() });
  await shot(page, "05-dashboard");

  // ---------------------------------------------------------------- 3. Scan hochladen (OCR)
  const b64 = fs.readFileSync(scanPdf).toString("base64");
  const up = await page.evaluate(async ({ b64 }) => {
    const bin = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const fd = new FormData();
    fd.append("document", new Blob([bin], { type: "application/pdf" }), "scan-zahnarzt.pdf");
    fd.append("title", "Scan Zahnarzt E2E");
    const csrf = (document.cookie.match(/(?:^|; )csrftoken=([^;]+)/) || [])[1] || "";
    const r = await fetch("/api/documents/post_document/", { method: "POST", body: fd, headers: { "X-CSRFToken": csrf }, credentials: "same-origin" });
    return { status: r.status, body: await r.text() };
  }, { b64 });
  step("Upload", up);
  if (up.status !== 200) throw new Error("Upload fehlgeschlagen");
  const taskId = JSON.parse(up.body);
  const tUp = Date.now();
  let task;
  for (let i = 0; i < 360; i++) {
    // API v10 (default in 3.4.2): status lower case ("success"), related_document_ids; v9: "SUCCESS", related_document
    const r = await api(page, "GET", `/api/tasks/?task_id=${taskId}`);
    const list = r.json && (Array.isArray(r.json) ? r.json : r.json.results);
    task = list && list[0];
    if (task && ["success", "failure"].includes(String(task.status).toLowerCase())) break;
    await sleep(1000);
  }
  step("Aufgabe fertig", { status: task && task.status, result: task && (task.result_message || task.result), seconds: (Date.now() - tUp) / 1000 });
  if (!task || String(task.status).toLowerCase() !== "success") throw new Error("Verarbeitung fehlgeschlagen");
  const docId = Number(task.related_document ?? (task.related_document_ids || [])[0]);
  const doc = await api(page, "GET", `/api/documents/${docId}/`);
  const content = (doc.json && doc.json.content) || "";
  step("OCR-Inhalt", { docId, chars: content.length, excerpt: content.replace(/\s+/g, " ").slice(0, 200) });
  if (!/Quokka/i.test(content) || !/Zahnarzt/i.test(content)) throw new Error("OCR-Text fehlt");

  // ---------------------------------------------------------------- 3b. text file: thumbnail with font (Z3)
  // Without a font (PAPERLESS_THUMBNAIL_FONT_NAME) the text file would get the default image from upstream.
  const txt = await page.evaluate(async () => {
    const fd = new FormData();
    fd.append("document", new Blob(["Notiz Z3\nVorschaubild mit Liberation Serif\nKennwort Wombatwolke\n"], { type: "text/plain" }), "notiz-z3.txt");
    const csrf = (document.cookie.match(/(?:^|; )csrftoken=([^;]+)/) || [])[1] || "";
    const r = await fetch("/api/documents/post_document/", { method: "POST", body: fd, headers: { "X-CSRFToken": csrf }, credentials: "same-origin" });
    return { status: r.status, body: await r.text() };
  });
  if (txt.status !== 200) throw new Error("Upload Textdatei fehlgeschlagen");
  const txtTask = JSON.parse(txt.body);
  let tt;
  for (let i = 0; i < 120; i++) {
    const r = await api(page, "GET", `/api/tasks/?task_id=${txtTask}`);
    const list = r.json && (Array.isArray(r.json) ? r.json : r.json.results);
    tt = list && list[0];
    if (tt && ["success", "failure"].includes(String(tt.status).toLowerCase())) break;
    await sleep(1000);
  }
  if (!tt || String(tt.status).toLowerCase() !== "success") throw new Error("Textdatei nicht verarbeitet");
  const txtId = Number(tt.related_document ?? (tt.related_document_ids || [])[0]);
  const thumb = await page.evaluate(async (id) => {
    const r = await fetch(`/api/documents/${id}/thumb/`, { credentials: "same-origin" });
    const b = new Uint8Array(await r.arrayBuffer());
    let s = ""; for (const x of b) s += String.fromCharCode(x);
    return { status: r.status, type: r.headers.get("content-type"), b64: btoa(s) };
  }, txtId);
  const thumbBytes = Buffer.from(thumb.b64, "base64");
  const defaultThumb = fs.readFileSync(path.join(repo, "upstream", "src", "documents", "resources", "document.webp"));
  const isDefault = thumbBytes.equals(defaultThumb);
  step("Textdatei-Vorschaubild", { docId: txtId, status: thumb.status, type: thumb.type, bytes: thumbBytes.length, standardbild: isDefault });
  fs.writeFileSync(path.join(outDir, "06b-text-vorschaubild.webp"), thumbBytes);
  if (thumb.status !== 200 || isDefault) throw new Error("Textdatei hat kein eigenes Vorschaubild (Schrift fehlt?)");

  // ---------------------------------------------------------------- 4. search
  const search = await api(page, "GET", "/api/documents/?query=Quokkaflausch");
  step("Volltextsuche (API)", { status: search.status, count: search.json && search.json.count });
  if (!search.json || search.json.count < 1) throw new Error("Suche findet das Dokument nicht");
  await page.goto(`${base}/rs/app/documents?query=Quokkaflausch`);
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(2500);
  await shot(page, "06-suche");
  await page.goto(`${base}/rs/app/documents/${docId}/details`);
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(2500);
  await shot(page, "07-dokument");

  // ---------------------------------------------------------------- 5. backup during operation
  const bk = desktopCli(homeDir, ["backup"]);
  // last line: "Sicherung (laufende Instanz) fertig: <Ordner>"
  const bkLine = bk.out.split(/\r?\n/).pop();
  const bkDir = bkLine.slice(bkLine.indexOf(": ") + 2).trim();
  const manifest = path.join(bkDir, "export", "manifest.json");
  const dump = path.join(bkDir, "paperrust.pgdump");
  step("Sicherung (laufende App)", {
    seconds: bk.ms / 1000, dir: bkDir,
    pgdump_bytes: fs.existsSync(dump) ? fs.statSync(dump).size : 0,
    manifest: fs.existsSync(manifest),
    export_files: fs.existsSync(path.join(bkDir, "export")) ? fs.readdirSync(path.join(bkDir, "export")).length : 0,
  });
  if (!fs.existsSync(manifest) || !fs.existsSync(dump)) throw new Error("Sicherung unvollständig");

  // ---------------------------------------------------------------- 6. quit
  await browser.close().catch(() => {});
  const st = desktopCli(homeDir, ["stop"]);
  const code = await waitExit(app, 60000);
  await sleep(1500);
  const left = processesFor(homeDir);
  const logs = fs.readdirSync(path.join(homeDir, "logs"));
  step("beendet", { stop: st.out, seconds: st.ms / 1000, appExit: code, uebrig: left, logs });
  if (!logs.includes("server.log")) throw new Error("logs/server.log fehlt");
  if (left.length) throw new Error("Prozesse übrig: " + left.join(", "));

  // ---------------------------------------------------------------- 7. restart
  const cdp2 = cdp1 + 1;
  const tRe = Date.now();
  app = startApp(homeDir, cdp2);
  ({ browser, page } = await connect(cdp2));
  await page.waitForURL(/http:\/\/127\.0\.0\.1:\d+\//, { timeout: 180000 });
  await page.waitForLoadState("networkidle").catch(() => {});
  step("Neustart bereit", { seconds: (Date.now() - tRe) / 1000, url: page.url() });
  if (/accounts\/login/.test(page.url())) {
    await page.fill("input[name=login]", ADMIN.user);
    await page.fill("input[name=password]", ADMIN.pass);
    await Promise.all([page.waitForURL(/\/(dashboard|rs\/app\/)/, { timeout: 60000 }), page.click("button[type=submit]")]);
    // /rs/app/ redirects client-side to /rs/app/m/ in the PaperRust design (default) – only query afterwards
    await page.waitForLoadState("networkidle").catch(() => {});
    await sleep(1500);
    step("erneut angemeldet", { url: page.url() });
  } else step("Sitzung blieb gültig");
  const again = await api(page, "GET", "/api/documents/?query=Quokkaflausch");
  step("Suche nach Neustart", { count: again.json && again.json.count });
  if (!again.json || again.json.count < 1) throw new Error("Dokument nach Neustart nicht gefunden");
  await page.goto(`${base}/rs/app/documents?query=Quokkaflausch`);
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(2500);
  await shot(page, "08-nach-neustart");
  await browser.close().catch(() => {});
  desktopCli(homeDir, ["stop"]);
  step("zweites Beenden", { appExit: await waitExit(app, 60000), uebrig: processesFor(homeDir) });

  // ---------------------------------------------------------------- 8. error page (DB does not start)
  const badHome = homeDir + "-fehler";
  fs.mkdirSync(path.join(badHome, "pgdata"), { recursive: true });
  fs.writeFileSync(path.join(badHome, "pgdata", "PG_VERSION"), "16\n");
  const cdp3 = cdp1 + 2;
  app = startApp(badHome, cdp3);
  ({ browser } = await connect(cdp3));
  // On a startup error the shell rebuilds the window (back to the local page): look for a new page.
  page = null;
  for (let i = 0; i < 240 && !page; i++) {
    for (const p of browser.contexts().flatMap((c) => c.pages())) {
      if (await p.$("#error-view:not([hidden])").catch(() => null)) { page = p; break; }
    }
    if (!page) await sleep(500);
  }
  if (!page) throw new Error("Fehlerseite erschien nicht");
  step("Fehlerseite", { error: await page.textContent("#error"), advice: await page.textContent("#advice"), log: await page.textContent("#log-link") });
  await shot(page, "09-fehlerseite");
  await page.click("#quit");
  step("Fehlerseite: Beenden", { appExit: await waitExit(app, 30000) });
  await browser.close().catch(() => {});
}

main().then(() => { step("E2E bestanden"); saveLog(true); process.exit(0); })
  .catch((e) => {
    step("E2E FEHLER", { error: String(e && e.stack || e) });
    saveLog(false);
    // Clean up: stop running instances in an orderly way (server, PostgreSQL)
    for (const h of [homeDir, homeDir + "-fehler"]) {
      try { desktopCli(h, ["stop"], 120000); } catch {}
    }
    process.exit(1);
  });
