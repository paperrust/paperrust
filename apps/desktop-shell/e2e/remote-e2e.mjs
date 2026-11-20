// PaperRust Desktop – E2E remote mode 2a (docs/REMOTE-APP.md §8, docs/DESKTOP.md §22).
//
//   node apps/desktop-shell/e2e/remote-e2e.mjs <install folder> <work folder> <result folder>
//
// Environment:
//   E2E_SRV        bash script of the local PaperRust test server (`<srv> start|stop`), default URL
//                  E2E_PR_URL (http://127.0.0.1:8191). Only a local test server on a DB copy.
//   E2E_NGX_URL    reference Django (paperless-ngx 3.4.2, http://127.0.0.1:8000) – read-only:
//                  detection + login page, no login (that would write last_login/session).
//   E2E_TLS_PORT   port of the TLS front in front of the PaperRust test server (default 8443).
//
// Controls the real Tauri window via WebView2 CDP (like apps/desktop/e2e/desktop-e2e.mjs). The app
// starts with a fresh PAPERRUST_DESKTOP_HOME – thereby its own identifier (single instance, WebView folder)
// and no contact with an installed app. Processes are only terminated by PID.
import { createRequire } from "node:module";
import { spawn, spawnSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..", "..");
const require = createRequire(path.join(repo, "tests", "e2e", "package.json"));
const { chromium } = require("playwright");

const [installDir, workDir, outDir] = process.argv.slice(2).map((p) => p && path.resolve(p));
if (!installDir || !workDir || !outDir) {
  console.error("usage: node remote-e2e.mjs <install> <work> <out>");
  process.exit(2);
}
const SRV = process.env.E2E_SRV;
const PR_URL = (process.env.E2E_PR_URL || "http://127.0.0.1:8191").replace(/\/$/, "");
const NGX_URL = (process.env.E2E_NGX_URL || "http://127.0.0.1:8000").replace(/\/$/, "");
const TLS_PORT = Number(process.env.E2E_TLS_PORT || 8443);
const TLS_URL = `https://127.0.0.1:${TLS_PORT}`;
const ONLY = (process.env.E2E_ONLY || "").split(",").filter(Boolean);
fs.mkdirSync(outDir, { recursive: true });
fs.rmSync(workDir, { recursive: true, force: true });
fs.mkdirSync(workDir, { recursive: true });
const home = path.join(workDir, "home");
const exe = path.join(installDir, "paperrust-app.exe");
const cdpPort = 9600 + Math.floor(Math.random() * 300);

const log = [];
const t0 = Date.now();
function step(name, data = {}) {
  const e = { t: ((Date.now() - t0) / 1000).toFixed(1), step: name, ...data };
  log.push(e);
  console.log(`[${e.t}s] ${name}`, Object.keys(data).length ? JSON.stringify(data) : "");
}
const checks = [];
function check(name, ok, detail = {}) {
  checks.push({ name, ok: !!ok, ...detail });
  step((ok ? "OK   " : "FAIL ") + name, detail);
  if (!ok) throw new Error("Prüfung fehlgeschlagen: " + name);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function save(ok, err) {
  fs.writeFileSync(
    path.join(outDir, "e2e-log.json"),
    JSON.stringify({ ok, error: err ? String(err.stack || err) : undefined, installDir, home, prUrl: PR_URL, ngxUrl: NGX_URL, tlsUrl: TLS_URL, checks, steps: log }, null, 2),
  );
}

function env() {
  const e = { ...process.env };
  for (const k of Object.keys(e)) if (/^(PAPERRUST_|PAPERLESS_|PG|WEBVIEW2_)/.test(k)) delete e[k];
  e.PAPERRUST_DESKTOP_HOME = home;
  e.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${cdpPort}`;
  // Tests do not query an update server (E2E_UPDATES=on switches the search back on)
  e.PAPERRUST_DESKTOP_UPDATES = process.env.E2E_UPDATES || "off";
  return e;
}

function ps(script) {
  return execFileSync("powershell.exe", ["-NoProfile", "-Command", script], { encoding: "utf8" });
}
function appPids() {
  const out = ps(`Get-CimInstance Win32_Process -Filter "Name='paperrust-app.exe'" | Where-Object { $_.ExecutablePath -eq '${exe.replace(/'/g, "''")}' } | ForEach-Object { $_.ProcessId }`);
  return out.split(/\r?\n/).filter(Boolean).map(Number);
}
function leftovers() {
  const h = home.replace(/'/g, "''");
  const i = installDir.replace(/'/g, "''");
  const out = ps(`Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and ($_.CommandLine.Contains('${h}') -or $_.ExecutablePath -like '${i}*') } | ForEach-Object { "$($_.ProcessId) $($_.Name)" }`);
  return out.split(/\r?\n/).filter(Boolean);
}

function startApp() {
  const child = spawn(exe, [], { env: env(), stdio: "ignore" });
  step("App gestartet", { pid: child.pid });
  return child;
}

function desktopCli(args) {
  const r = spawnSync(path.join(installDir, "paperrust-desktop.exe"), args, { env: env(), encoding: "utf8", timeout: 120000 });
  return { status: r.status, out: `${r.stdout || ""}${r.stderr || ""}`.trim() };
}

function srv(cmd) {
  if (!SRV) throw new Error("E2E_SRV fehlt");
  const r = spawnSync("bash", [SRV, cmd], { encoding: "utf8", timeout: 180000 });
  step(`Testserver ${cmd}`, { status: r.status, out: (r.stdout || "").trim().slice(-200) });
  if (r.status !== 0) throw new Error(`srv ${cmd}: ${r.stderr}`);
}

let browser = null;
let page = null;

async function connect() {
  for (let i = 0; i < 160; i++) {
    try {
      const r = await fetch(`http://127.0.0.1:${cdpPort}/json/version`);
      if (r.ok) break;
    } catch {}
    await sleep(250);
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  for (let i = 0; i < 80; i++) {
    const pages = browser.contexts().flatMap((c) => c.pages());
    if (pages.length) {
      page = pages[0];
      page.on("dialog", (d) => d.accept());
      return;
    }
    await sleep(250);
  }
  throw new Error("kein WebView-Fenster gefunden");
}

/// After a restart of the app: old PID gone, new one there, reconnect.
async function reconnectAfterRestart(oldPids, what) {
  const t = Date.now();
  try { await browser.close(); } catch {}
  let pids = [];
  for (let i = 0; i < 120; i++) {
    pids = appPids();
    if (pids.length && pids.every((p) => !oldPids.includes(p))) break;
    await sleep(250);
  }
  check(`${what}: App neu gestartet`, pids.length && pids.every((p) => !oldPids.includes(p)), { old: oldPids, now: pids, ms: Date.now() - t });
  await connect();
  return pids;
}

async function shot(name) {
  const f = path.join(outDir, name + ".png");
  await page.screenshot({ path: f });
  step("Screenshot", { file: path.relative(repo, f) });
}

async function waitUrl(re, timeout = 60000) {
  const t = Date.now();
  while (Date.now() - t < timeout) {
    try { if (re.test(page.url())) return page.url(); } catch {}
    await sleep(200);
  }
  throw new Error(`URL ${re} nicht erreicht, ist ${page.url()}`);
}

function appToml() {
  return fs.readFileSync(path.join(home, "app.toml"), "utf8");
}
function profiles() {
  // very small TOML reader for [[profile]] (id, name, url, flavor, tls_pin, allow_http)
  const out = [];
  let cur = null;
  for (const line of appToml().split(/\r?\n/)) {
    if (line.trim() === "[[profile]]") { cur = {}; out.push(cur); continue; }
    const m = line.match(/^(\w+)\s*=\s*(.*)$/);
    if (!m) continue;
    const v = m[2].startsWith('"') ? JSON.parse(m[2]) : m[2];
    if (cur) cur[m[1]] = v; else out[m[1]] = v;
  }
  return out;
}

async function login(user, pass) {
  await page.waitForSelector("input[name=login]", { timeout: 60000 });
  await page.fill("input[name=login]", user);
  await page.fill("input[name=password]", pass);
  await Promise.all([page.waitForURL(/\/(dashboard|rs\/app\/)/, { timeout: 60000 }), page.click("button[type=submit]")]);
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(1500);
}

async function connectForm(url, name, { expectHttp = false, expectTls = false } = {}) {
  await page.waitForSelector("#fields-remote:not([hidden])", { timeout: 30000 });
  await page.fill("input[name=server_url]", url);
  await page.fill("input[name=profile_name]", name);
  await page.click("#submit");
  let tlsInfo = null;
  if (expectHttp) {
    await page.waitForSelector("#http-confirm:not([hidden])", { timeout: 30000 });
    step("http-Bestätigung verlangt", { msg: await page.textContent("#http-message") });
    await page.check("input[name=allow_http]");
    await page.click("#submit");
  }
  if (expectTls) {
    await page.waitForSelector("#tls-confirm:not([hidden])", { timeout: 30000 });
    tlsInfo = {
      message: await page.textContent("#tls-message"),
      subject: await page.textContent("#cert-subject"),
      fingerprint: (await page.textContent("#cert-fp")).trim(),
      until: await page.textContent("#cert-until"),
    };
    return tlsInfo;
  }
  return null;
}

async function waitConnected() {
  await page.waitForFunction(() => /PaperRust startet neu/.test(document.getElementById("stage")?.textContent || ""), null, { timeout: 30000 });
  return (await page.textContent("#stage")).trim();
}

function openssl(args, input) {
  return execFileSync("openssl", args, { input, encoding: input ? undefined : "utf8" });
}

function makeCert(name) {
  const key = path.join(workDir, `${name}.key`);
  const crt = path.join(workDir, `${name}.pem`);
  execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", crt, "-days", "30", "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1"], { stdio: "ignore" });
  const fp = openssl(["x509", "-noout", "-fingerprint", "-sha256", "-in", crt]).trim().split("=")[1];
  const pub = execFileSync("openssl", ["x509", "-pubkey", "-noout", "-in", crt]);
  const der = execFileSync("openssl", ["pkey", "-pubin", "-outform", "der"], { input: pub });
  const spki = crypto.createHash("sha256").update(der).digest("hex");
  return { key, crt, fp, spki };
}

let proxy = null;
async function startProxy(cert) {
  proxy = spawn(process.execPath, [path.join(here, "tls-proxy.mjs"), String(TLS_PORT), PR_URL.replace(/^https?:\/\//, ""), cert.crt, cert.key], { stdio: "ignore" });
  await sleep(800);
  step("TLS-Vorbau gestartet", { pid: proxy.pid, port: TLS_PORT });
}
function stopProxy() {
  if (proxy && proxy.exitCode === null) {
    spawnSync("taskkill", ["/PID", String(proxy.pid), "/F"]);
    step("TLS-Vorbau gestoppt", { pid: proxy.pid });
  }
  proxy = null;
}

async function openSettings() {
  await page.goto("http://tauri.localhost/settings.html");
  await page.waitForSelector("#home:not(:empty)", { timeout: 15000 });
  await sleep(300);
}

async function stopApp(what) {
  const pids = appPids();
  const t = Date.now();
  const r = desktopCli(["stop"]);
  for (let i = 0; i < 120 && appPids().length; i++) await sleep(250);
  const rest = appPids();
  if (rest.length) for (const p of rest) spawnSync("taskkill", ["/PID", String(p), "/T", "/F"]);
  check(`${what}: Beenden über paperrust-desktop stop`, rest.length === 0, { pids, cli: r.status, ms: Date.now() - t });
  try { await browser.close(); } catch {}
}

async function main() {
  // ================================================================ A. first start → PaperRust
  let app = startApp();
  await connect();
  await page.waitForSelector("#setup-form:not([hidden])", { timeout: 60000 });
  check("Erst-Start zeigt Wahl lokal/Server", await page.isVisible("#choice-remote") && await page.isVisible("#choice-fresh"));
  await shot("01-erststart-wahl");
  await page.check("input[name=mode][value=remote]");
  await shot("02-verbinden-formular");

  // invalid inputs
  for (const [bad, expect] of [["https://user@127.0.0.1:8191", /Benutzername/], ["http://example.com", /nur für diesen Computer/], ["ftp://127.0.0.1", /Nur https/]]) {
    await page.fill("input[name=server_url]", bad);
    await page.click("#submit");
    await page.waitForFunction(() => (document.getElementById("form-error").textContent || "").length > 0, null, { timeout: 15000 });
    const msg = await page.textContent("#form-error");
    check(`abgelehnt: ${bad}`, expect.test(msg), { msg });
  }
  await shot("03-http-oeffentlich-abgelehnt");

  await connectForm(PR_URL, "PaperRust-Test", { expectHttp: false });
  await page.waitForSelector("#http-confirm:not([hidden])", { timeout: 30000 });
  await shot("04-http-bestaetigung");
  await page.click("#submit"); // without the checkbox ticked
  check("http ohne Bestätigung nicht verbunden", /bestätigen/.test(await page.textContent("#form-error")));
  await page.check("input[name=allow_http]");
  let pids = appPids();
  await page.click("#submit");
  const msgA = await waitConnected();
  check("Erkennung PaperRust", /PaperRust/.test(msgA) && !/paperless-ngx/.test(msgA), { msg: msgA });
  await reconnectAfterRestart(pids, "Verbinden");
  await waitUrl(new RegExp("^" + PR_URL.replace(/[.:/]/g, "\\$&") + "/accounts/login/"), 60000);
  await page.waitForSelector("input[name=login]");
  await shot("05-login-paperrust");
  let pr = profiles();
  const prProfile = pr[0];
  check("app.toml: remote, Profil PaperRust", pr.mode === '"remote"' || appToml().includes('mode = "remote"'), { profile: prProfile });
  check("app.toml: flavor/allow_http", prProfile.flavor === "paperrust" && prProfile.allow_http === "true" && prProfile.url === PR_URL + "/", { profile: prProfile });
  const prWebview = path.join(home, "profiles", prProfile.id, "webview");
  check("eigenes WebView-Datenverzeichnis je Profil", fs.existsSync(prWebview), { dir: prWebview, entries: fs.existsSync(prWebview) ? fs.readdirSync(prWebview) : [] });
  check("Fern-Modus: kein pgdata, kein desktop.toml", !fs.existsSync(path.join(home, "pgdata")) && !fs.existsSync(path.join(home, "desktop.toml")));
  check("Fern-Modus: kein PostgreSQL/Server-Prozess", leftovers().every((l) => /paperrust-app|msedgewebview2/i.test(l)), { procs: leftovers() });

  await login("admin", "admin");
  await shot("06-dashboard-paperrust");
  await page.goto(PR_URL + "/rs/app/documents");
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(2500);
  await shot("07-dokumentliste-paperrust");
  const count = await page.evaluate(async () => (await (await fetch("/api/documents/?page_size=1", { headers: { Accept: "application/json" } })).json()).count);
  check("Dokumentliste über das Fenster (Session-Cookie)", count === 195, { count });

  // Security: server page must not execute Tauri commands
  const sec = await page.evaluate(async () => {
    const r = { tauri: typeof window.__TAURI__, internals: typeof window.__TAURI_INTERNALS__ };
    for (const cmd of ["settings_state", "shell_state", "profile_remove"]) {
      try {
        const f = window.__TAURI__?.core?.invoke || window.__TAURI_INTERNALS__?.invoke;
        if (!f) { r[cmd] = "kein invoke"; continue; }
        const v = await f(cmd, { id: "x" });
        r[cmd] = "ERLAUBT: " + JSON.stringify(v).slice(0, 80);
      } catch (e) { r[cmd] = "abgelehnt: " + String(e).slice(0, 120); }
    }
    return r;
  });
  check("Server-Seite: IPC abgelehnt", ["settings_state", "shell_state", "profile_remove"].every((c) => !String(sec[c]).startsWith("ERLAUBT")), sec);
  await page.evaluate(() => { location.href = "ftp://127.0.0.1/x"; });
  await sleep(1500);
  check("Navigation zu fremdem Schema bleibt im Fenster aus", page.url().startsWith(PR_URL), { url: page.url() });

  // Offline page
  srv("stop");
  const tOff = Date.now();
  await page.reload().catch(() => {});
  await waitUrl(/tauri\.localhost\/index\.html/, 45000);
  await page.waitForSelector("#offline-view:not([hidden])", { timeout: 30000 });
  await sleep(1200);
  await shot("08-offline");
  check("Offline-Seite", /nicht erreichbar/.test(await page.textContent("#offline-view")), {
    ms: Date.now() - tOff,
    error: await page.textContent("#offline-error"),
    last: await page.textContent("#offline-last"),
    next: await page.textContent("#offline-next"),
  });
  srv("start");
  const tOn = Date.now();
  await page.click("#offline-retry");
  await waitUrl(new RegExp("^" + PR_URL.replace(/[.:/]/g, "\\$&")), 30000);
  await page.waitForLoadState("networkidle").catch(() => {});
  await sleep(2000);
  await shot("09-wieder-verbunden");
  check("Erneut versuchen → wieder verbunden (Sitzung noch da)", /dashboard|documents|rs\/app\//.test(page.url()), { url: page.url(), ms: Date.now() - tOn });

  // ================================================================ B. settings, ngx profile
  await openSettings();
  await shot("10-einstellungen");
  // E2E_SKIP_NGX=1: no reference Django reachable → skip part B (ngx profile, profile switch)
  if (process.env.E2E_SKIP_NGX === "1") step("Teil B übersprungen (E2E_SKIP_NGX=1, kein Referenz-Django)");
  else {
  await page.click("#add-profile");
  await connectForm(NGX_URL, "Referenz ngx", { expectHttp: true });
  pids = appPids();
  const msgB = await waitConnected();
  check("Erkennung paperless-ngx", /paperless-ngx 3\.4\.2/.test(msgB), { msg: msgB });
  await reconnectAfterRestart(pids, "Profil ngx");
  await waitUrl(new RegExp("^" + NGX_URL.replace(/[.:/]/g, "\\$&") + "/accounts/login/"), 60000);
  await page.waitForSelector("input[name=login]");
  await sleep(1000);
  await shot("11-login-ngx");
  pr = profiles();
  const ngx = pr.find((p) => p.url === NGX_URL + "/");
  check("zweites Profil ngx aktiv", ngx && ngx.flavor === "ngx" && appToml().includes(`active_profile = "${ngx.id}"`), { ngx });
  check("Cookies je Profil getrennt (eigener Ordner)", fs.existsSync(path.join(home, "profiles", ngx.id, "webview")) && ngx.id !== prProfile.id);

  // back to PaperRust via "Verwenden"
  await openSettings();
  pids = appPids();
  await page.click(`li[data-id="${prProfile.id}"] button:has-text("Verwenden")`);
  await reconnectAfterRestart(pids, "Profilwechsel");
  await waitUrl(new RegExp("^" + PR_URL.replace(/[.:/]/g, "\\$&")), 60000);
  await page.waitForLoadState("networkidle").catch(() => {});
  step("nach Profilwechsel", { url: page.url() });
  if (/accounts\/login/.test(page.url())) await login("admin", "admin");
  }

  // ================================================================ C. log out
  const sentinel = path.join(prWebview, "e2e-sentinel.txt");
  fs.writeFileSync(sentinel, "vor dem Abmelden");
  await openSettings();
  pids = appPids();
  await page.click(`li[data-id="${prProfile.id}"] button:has-text("Abmelden")`);
  await reconnectAfterRestart(pids, "Abmelden");
  await waitUrl(new RegExp("^" + PR_URL.replace(/[.:/]/g, "\\$&") + "/accounts/login/"), 60000);
  await shot("12-nach-abmelden");
  check("Abmelden löscht WebView-Daten des Profils", !fs.existsSync(sentinel) && fs.existsSync(prWebview), { sentinel: fs.existsSync(sentinel) });

  // ================================================================ D. TLS (self-signed, pin)
  const c1 = makeCert("cert1");
  const c2 = makeCert("cert2");
  await startProxy(c1);
  await openSettings();
  await page.click("#add-profile");
  const tls = await connectForm(TLS_URL, "TLS-Test", { expectTls: true });
  await shot("13-tls-dialog");
  check("TLS-Dialog zeigt den echten Fingerabdruck", tls.fingerprint === c1.fp, { shown: tls.fingerprint, openssl: c1.fp, subject: tls.subject });
  await page.click("#submit");
  check("ohne Haken nicht verbunden", /Fingerabdruck/.test(await page.textContent("#form-error")));
  await page.check("input[name=tls_ok]");
  pids = appPids();
  await page.click("#submit");
  const msgD = await waitConnected();
  check("Erkennung über TLS mit Pin", /PaperRust/.test(msgD), { msg: msgD });
  await reconnectAfterRestart(pids, "TLS-Profil");
  const tlsProfile = profiles().find((p) => p.url === TLS_URL + "/");
  check("tls_pin = SPKI-SHA-256 des Zertifikats", tlsProfile && tlsProfile.tls_pin === c1.spki, { pin: tlsProfile?.tls_pin, openssl: c1.spki });
  await waitUrl(new RegExp("^https://127\\.0\\.0\\.1:" + TLS_PORT + "/accounts/login/"), 60000);
  await page.waitForSelector("input[name=login]");
  await shot("14-login-tls-selbstsigniert");
  check("WebView2 lädt selbst signierte Seite (ServerCertificateErrorDetected + Pin)", true, { url: page.url() });
  await login("admin", "admin");
  await shot("15-dashboard-tls");

  // certificate changes → hard error
  stopProxy();
  await startProxy(c2);
  const tSwap = Date.now();
  await page.reload().catch(() => {});
  await waitUrl(/tauri\.localhost\/index\.html/, 60000);
  await page.waitForSelector("#error-view:not([hidden])", { timeout: 60000 });
  await shot("16-zertifikat-geaendert");
  check("geändertes Zertifikat → Verbindung abgelehnt", /Zertifikat/.test(await page.textContent("#error")), { msg: await page.textContent("#error"), ms: Date.now() - tSwap });
  stopProxy();
  await startProxy(c1);
  await page.click("#retry");
  await waitUrl(/^https:\/\/127\.0\.0\.1/, 30000);
  step("altes Zertifikat zurück → wieder verbunden", { url: page.url() });

  // ================================================================ E. mode switch local ↔ remote
  await openSettings();
  // E2E_DESKTOP_TOML: prepared desktop.toml (own ports) for the local mode
  if (process.env.E2E_DESKTOP_TOML) fs.copyFileSync(process.env.E2E_DESKTOP_TOML, path.join(home, "desktop.toml"));
  await page.check("input[name=mode][value=local]");
  pids = appPids();
  await page.click("#apply-mode");
  await reconnectAfterRestart(pids, "Wechsel → lokal");
  await page.waitForSelector("#setup-form:not([hidden])", { timeout: 240000 });
  check("lokal: Supervisor fragt Erst-Start (ohne Server-Option)", !(await page.isVisible("#choice-remote")));
  await shot("17-lokal-erststart");
  // "Start fresh" without user name/password: automatic login as local owner
  await page.check("input[name=mode][value=fresh]");
  await page.click("#submit");
  await waitUrl(/http:\/\/127\.0\.0\.1:\d+\/(dashboard|rs\/app\/)/, 300000);
  const localBase = new URL(page.url()).origin;
  await shot("18-lokal-dashboard");
  const pgv = path.join(home, "pgdata", "PG_VERSION");
  check("lokal läuft, pgdata angelegt", fs.existsSync(pgv), { localBase });

  await openSettings();
  await page.check("input[name=mode][value=remote]");
  pids = appPids();
  await page.click("#apply-mode");
  await reconnectAfterRestart(pids, "Wechsel → fern");
  await waitUrl(/^https:\/\/127\.0\.0\.1/, 90000);
  await shot("19-wieder-fern");
  check("fern: lokale Daten unberührt, PostgreSQL gestoppt", fs.existsSync(pgv) && !leftovers().some((l) => /postgres|paperrust\.exe/i.test(l)), { procs: leftovers() });
  await stopApp("Fern-Modus");

  app = startApp();
  await connect();
  await waitUrl(/^https:\/\/127\.0\.0\.1/, 60000);
  await openSettings();
  await page.check("input[name=mode][value=local]");
  pids = appPids();
  await page.click("#apply-mode");
  await reconnectAfterRestart(pids, "Wechsel → lokal (2)");
  // automatic login as local owner (no login form)
  await waitUrl(/http:\/\/127\.0\.0\.1:\d+\/(dashboard|rs\/app\/)/, 180000);
  await shot("20-lokal-daten-erhalten");
  check("lokal: Daten nach Hin- und Rückwechsel erhalten (automatisch angemeldet)", /dashboard|rs\/app\//.test(page.url()));
  await stopApp("lokaler Modus");
  const rest = leftovers();
  check("keine Prozesse übrig", rest.length === 0, { rest });
}

main()
  .then(() => { stopProxy(); save(true); console.log("E2E OK"); process.exit(0); })
  .catch(async (e) => {
    console.error(e);
    try { if (page) await shot("zz-fehler"); } catch {}
    stopProxy();
    save(false, e);
    for (const p of appPids()) spawnSync("taskkill", ["/PID", String(p), "/T", "/F"]);
    process.exit(1);
  });
