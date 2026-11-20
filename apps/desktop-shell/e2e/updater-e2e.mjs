// PaperRust Desktop – E2E automatic updates (Z4, docs/DESKTOP.md §23).
//
//   node apps/desktop-shell/e2e/updater-e2e.mjs <installer folder> <work folder> <result folder>
//
// <installer folder> (from updater-e2e.ps1): base-setup.exe (0.1.0), update-setup.exe (0.1.1-test)
// + update-setup.exe.sig (test key, version bound). Both installers: product
// "PaperRust Updatetest" (own registry keys), debug shell with test pubkey.
//
// Flow: local HTTP server (127.0.0.1) serves stable.json/beta.json + files → silent base install into
// <work folder>/install → start app locally (fresh data folder, account via environment) → search
// stable (nothing) → channel beta in the settings → manipulated file rejected → correct file
// downloaded + checked, update window → manipulated on disk: installing refused → downloaded
// again → "Jetzt neu starten und installieren": app stops server + PostgreSQL in an orderly way, installer
// runs, helper starts the new version with the same environment → version/registry/data checked →
// stop → uninstall. Controlled via WebView2 CDP (like remote-e2e.mjs). Processes only by PID.
import { createRequire } from "node:module";
import { spawn, spawnSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..", "..");
const require = createRequire(path.join(repo, "tests", "e2e", "package.json"));
const { chromium } = require("playwright");

const [instDir, workDir, outDir] = process.argv.slice(2).map((p) => p && path.resolve(p));
if (!instDir || !workDir || !outDir) {
  console.error("usage: node updater-e2e.mjs <installers> <work> <out>");
  process.exit(2);
}
const UPD = process.env.E2E_UPDATE_VERSION || "0.1.1-test";
const PRODUCT = "PaperRust Updatetest";
const UNINST_KEY = `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\${PRODUCT}`;
fs.mkdirSync(outDir, { recursive: true });
fs.rmSync(workDir, { recursive: true, force: true });
fs.mkdirSync(workDir, { recursive: true });
const installDir = path.join(workDir, "install");
const home = path.join(workDir, "home");
const exe = path.join(installDir, "paperrust-app.exe");
const cdpPort = 9300 + Math.floor(Math.random() * 300);
const ADMIN = { user: "admin", pass: "Upd4te-Test-" + crypto.randomBytes(4).toString("hex") };

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
async function waitFor(what, fn, timeout = 60000, every = 500) {
  const t = Date.now();
  while (Date.now() - t < timeout) {
    try {
      const v = await fn();
      if (v) return v;
    } catch {}
    await sleep(every);
  }
  throw new Error(`Zeitüberschreitung: ${what}`);
}
function save(ok, err) {
  fs.writeFileSync(
    path.join(outDir, "e2e-log.json"),
    JSON.stringify({ ok, error: err ? String(err.stack || err) : undefined, update: UPD, installDir, home, requests, checks, steps: log }, null, 2),
  );
}
const sha = (b) => crypto.createHash("sha256").update(b).digest("hex");
const readJson = (f) => JSON.parse(fs.readFileSync(f, "utf8"));
const updDir = path.join(home, "updates");
const stateFile = path.join(updDir, "state.json");
const pendingFile = path.join(updDir, "pending.json");
const state = () => (fs.existsSync(stateFile) ? readJson(stateFile) : {});
// desktop.log + desktop.log.1 (the supervisor rotates at start)
const desktopLog = () =>
  ["desktop.log.1", "desktop.log"]
    .map((n) => path.join(home, "logs", n))
    .filter((f) => fs.existsSync(f))
    .map((f) => fs.readFileSync(f, "utf8"))
    .join("\n");

// ---------------------------------------------------------------- processes
function ps(script) {
  return execFileSync("powershell.exe", ["-NoProfile", "-Command", script], { encoding: "utf8" });
}
function appPids() {
  const out = ps(`Get-CimInstance Win32_Process -Filter "Name='paperrust-app.exe'" | Where-Object { $_.ExecutablePath -eq '${exe.replace(/'/g, "''")}' } | ForEach-Object { $_.ProcessId }`);
  return out.split(/\r?\n/).filter(Boolean).map(Number);
}
function procs(filter) {
  const h = home.replace(/'/g, "''");
  const i = installDir.replace(/'/g, "''");
  const out = ps(`Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.Name -ne 'node.exe' -and $_.Name -ne 'powershell.exe' -and (($_.CommandLine -and $_.CommandLine.Contains('${h}')) -or ($_.ExecutablePath -and ($_.ExecutablePath -like '${i}*' -or $_.ExecutablePath -like '${h}*'))) } | ForEach-Object { "$($_.ProcessId) $($_.Name)" }`);
  return out.split(/\r?\n/).filter(Boolean).filter((l) => !filter || filter.test(l));
}
function env() {
  const e = { ...process.env };
  for (const k of Object.keys(e)) if (/^(PAPERRUST_|PAPERLESS_|PG|WEBVIEW2_|TAURI_)/.test(k)) delete e[k];
  e.PAPERRUST_DESKTOP_HOME = home;
  e.PAPERRUST_DESKTOP_ADMIN_USER = ADMIN.user;
  e.PAPERRUST_DESKTOP_ADMIN_EMAIL = "admin@example.invalid";
  e.PAPERRUST_DESKTOP_ADMIN_PASSWORD = ADMIN.pass;
  e.PAPERRUST_DESKTOP_UPDATE_URL = `${base}/{{channel}}.json`;
  e.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${cdpPort}`;
  return e;
}
function reg(key, value) {
  const r = spawnSync("reg", ["query", key, "/v", value], { encoding: "utf8" });
  if (r.status !== 0) return null;
  const m = r.stdout.match(new RegExp(`${value}\\s+REG_\\w+\\s+(.*)`));
  return m ? m[1].trim() : null;
}

// ---------------------------------------------------------------- update server
const files = {
  base: fs.readFileSync(path.join(instDir, "base-setup.exe")),
  update: fs.readFileSync(path.join(instDir, "update-setup.exe")),
};
const sig = fs.readFileSync(path.join(instDir, "update-setup.exe.sig"), "utf8").trim();
const tampered = Buffer.from(files.update);
tampered[Math.floor(tampered.length / 2)] ^= 0x01;
let phase = "tampered";
const requests = [];
let base = "";
function manifest(version, file) {
  const entry = { signature: sig, url: `${base}/files/${file}` };
  return JSON.stringify({ version, notes: `Testversion ${version}\n- Automatische Updates (E2E)\n<b>kein HTML</b>`, pub_date: "2026-11-09T12:00:00Z", platforms: { "windows-x86_64-nsis": entry, "windows-x86_64": entry } });
}
const server = http.createServer((req, res) => {
  requests.push({ t: ((Date.now() - t0) / 1000).toFixed(1), url: req.url, phase });
  let body = null;
  let type = "application/octet-stream";
  if (req.url === "/stable.json") { body = manifest("0.1.0", "update-setup.exe"); type = "application/json"; }
  else if (req.url === "/beta.json") { body = manifest(UPD, phase === "tampered" ? "update-tampered.exe" : "update-setup.exe"); type = "application/json"; }
  else if (req.url === "/files/update-setup.exe") body = files.update;
  else if (req.url === "/files/update-tampered.exe") body = tampered;
  if (body === null) { res.writeHead(404); res.end(); return; }
  res.writeHead(200, { "Content-Type": type, "Content-Length": Buffer.byteLength(body) });
  res.end(body);
});

// ---------------------------------------------------------------- CDP
let browser = null;
async function connect() {
  await waitFor("CDP", async () => (await fetch(`http://127.0.0.1:${cdpPort}/json/version`)).ok, 60000, 250);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
}
const pages = () => browser.contexts().flatMap((c) => c.pages());
async function pageWith(re, timeout = 30000) {
  return waitFor(`Seite ${re}`, () => pages().find((p) => re.test(p.url())), timeout, 250);
}
async function shot(p, name) {
  const f = path.join(outDir, name + ".png");
  await p.screenshot({ path: f });
  step("Screenshot", { file: path.relative(repo, f) });
}

// ---------------------------------------------------------------- flow
async function main() {
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${server.address().port}`;
  step("Update-Server", { base, update: UPD, size: files.update.length, sha256: sha(files.update) });
  check("keine fremde Installation „PaperRust Updatetest“ vorhanden", reg(UNINST_KEY, "DisplayVersion") === null);

  // 1) install base silently
  let r = spawnSync(path.join(instDir, "base-setup.exe"), ["/S", `/D=${installDir}`], { windowsVerbatimArguments: true, timeout: 300000 });
  check("Basis-Installer 0.1.0 still installiert", r.status === 0 && fs.existsSync(exe) && fs.existsSync(path.join(installDir, "uninstall.exe")), { status: r.status });
  check("Registry: DisplayVersion 0.1.0", reg(UNINST_KEY, "DisplayVersion") === "0.1.0");
  const exeHashBefore = sha(fs.readFileSync(exe));

  // 2) local start (channel stable via app.toml)
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(path.join(home, "app.toml"), 'mode = "local"\n\n[updates]\nchannel = "stable"\n');
  const child = spawn(exe, [], { env: env(), stdio: "ignore" });
  const pid1 = child.pid;
  step("App gestartet", { pid: pid1 });
  await waitFor("PaperRust bereit", () => /PaperRust bereit/.test(desktopLog()), 180000);
  const inst = fs.readFileSync(path.join(home, "run", "instance.toml"), "utf8");
  const appUrl = inst.match(/url\s*=\s*"([^"]+)"/)[1].replace(/\/?$/, "/");
  step("bereit", { appUrl });
  await waitFor("erste Suche (stable)", () => state().last_check_ms > 0, 90000);
  check("stable: kein Update (0.1.1-test ist Vorab-Version)", !state().last_error && !fs.existsSync(pendingFile) && /Kein Update verfügbar/.test(desktopLog()), { state: state() });
  check("stable: nur stable.json abgefragt", requests.some((q) => q.url === "/stable.json") && !requests.some((q) => q.url === "/beta.json"));

  // marker for "data stays untouched"
  const marker = crypto.randomBytes(16).toString("hex");
  fs.mkdirSync(path.join(home, "backups"), { recursive: true });
  fs.writeFileSync(path.join(home, "backups", "z4-marker.txt"), marker);
  const pgVersion = fs.readFileSync(path.join(home, "pgdata", "PG_VERSION"), "utf8");
  const auth = "Basic " + Buffer.from(`${ADMIN.user}:${ADMIN.pass}`).toString("base64");
  const api = async () => (await fetch(appUrl + "api/documents/", { headers: { Authorization: auth, Accept: "application/json; version=9" } })).status;
  check("API vor dem Update: Anmeldung admin", (await api()) === 200);

  // 3) channel beta via the settings; manipulated file is rejected
  await connect();
  const main = await pageWith(/127\.0\.0\.1|tauri\.localhost/);
  await main.goto("http://tauri.localhost/settings.html");
  await main.waitForSelector("#upd-status:not(:empty)", { timeout: 15000 });
  await main.check("input[name=channel][value=beta]");
  await main.click("#upd-save");
  await waitFor("Ablehnung der manipulierten Datei", () => /Signatur ungültig/.test(state().last_error || ""), 120000);
  check("beta: manipulierter Installer abgelehnt, nichts abgelegt", !fs.existsSync(pendingFile) && !fs.readdirSync(updDir).some((f) => f.endsWith("-setup.exe")) && requests.some((q) => q.url === "/files/update-tampered.exe"), { error: state().last_error });
  check("app.toml: Kanal beta gespeichert", /channel = "beta"/.test(fs.readFileSync(path.join(home, "app.toml"), "utf8")));
  await sleep(2500);
  await shot(main, "01-einstellungen-signatur-abgelehnt");

  // 4) correct file → downloaded, checked, update window
  phase = "good";
  await main.click("#upd-open");
  let upd = await pageWith(/update\.html/);
  await upd.waitForSelector("#check:not([disabled])", { timeout: 15000 });
  await upd.click("#check");
  await waitFor("Update abgelegt", () => fs.existsSync(pendingFile), 180000);
  let pend = readJson(pendingFile);
  check("Update geladen + Signatur geprüft", pend.version === UPD && pend.sha256 === sha(files.update) && sha(fs.readFileSync(path.join(updDir, pend.file))) === sha(files.update), { pending: { ...pend, manifest: undefined } });
  await upd.waitForSelector("#ready:not([hidden])", { timeout: 15000 });
  const notes = await upd.textContent("#notes");
  check("Update-Fenster zeigt Version + Versionshinweise als Text", (await upd.textContent("#new-version")) === UPD && notes.includes("<b>kein HTML</b>"));
  await shot(upd, "02-update-bereit");

  // 5) manipulated on disk → installing refused
  const f = path.join(updDir, pend.file);
  const b = fs.readFileSync(f);
  b[1000] ^= 0x01;
  fs.writeFileSync(f, b);
  await upd.click("#install");
  await waitFor("Fehlermeldung", async () => /Signatur ungültig/.test(await upd.textContent("#error")), 30000);
  check("manipulierte Datei auf der Platte: Einspielen verweigert, verworfen, App läuft weiter", !fs.existsSync(pendingFile) && !fs.existsSync(f) && appPids().includes(pid1) && /Signatur ungültig/.test(state().last_error || ""));
  await shot(upd, "03-einspielen-verweigert");

  // 6) download again
  await upd.waitForSelector("#check:not([disabled])", { timeout: 15000 });
  await upd.click("#check");
  await waitFor("Update erneut abgelegt", () => fs.existsSync(pendingFile), 180000);
  await upd.waitForSelector("#ready:not([hidden])", { timeout: 15000 });

  // 7) install
  const pgRunning = procs(/postgres\.exe/).length;
  check("vor dem Einspielen läuft PostgreSQL + Server", pgRunning > 0 && procs(/paperrust\.exe/).length === 1, { pg: pgRunning });
  const tInstall = Date.now();
  await upd.click("#install");
  try { await browser.close(); } catch {}
  await waitFor("alte App beendet", () => !appPids().includes(pid1), 90000, 250);
  const stopMs = Date.now() - tInstall;
  const left = procs(/postgres\.exe|paperrust\.exe /);
  const pgLog = fs.readFileSync(path.join(home, "logs", "postgresql.log"), "utf8");
  check("vor dem Installer: Server + PostgreSQL geordnet gestoppt", left.length === 0 && !fs.existsSync(path.join(home, "pgdata", "postmaster.pid")) && /database system is shut down/.test(pgLog.slice(-4000)), { left, stopMs });
  check("Update-Zustand: installing gesetzt", state().installing?.version === UPD);
  await waitFor("Registry DisplayVersion " + UPD, () => reg(UNINST_KEY, "DisplayVersion") === UPD, 300000, 1000);
  step("Installer fertig", { s: ((Date.now() - tInstall) / 1000).toFixed(1) });
  const pid2 = await waitFor("neue App (Helfer)", () => appPids().find((p) => p !== pid1), 120000, 500);
  step("neue App läuft", { pid: pid2 });
  await waitFor("Update eingespielt (Log)", () => new RegExp(`Update auf ${UPD.replace(/\./g, "\\.")} eingespielt`).test(desktopLog()), 180000);
  await waitFor("PaperRust bereit (nach Update)", () => (desktopLog().match(/PaperRust bereit/g) || []).length >= 2, 180000);
  check("Programm ersetzt", sha(fs.readFileSync(exe)) !== exeHashBefore);
  const st = state();
  check("Zustand aufgeräumt (kein installing/pending/Installer)", !st.installing && !st.failed_version && !fs.existsSync(pendingFile) && !fs.readdirSync(updDir).some((x) => x.endsWith("-setup.exe")), { st });
  check("Daten unberührt (Marker, PG_VERSION)", fs.readFileSync(path.join(home, "backups", "z4-marker.txt"), "utf8") === marker && fs.readFileSync(path.join(home, "pgdata", "PG_VERSION"), "utf8") === pgVersion);
  check("neue Version: gleicher Datenordner (Helfer behält PAPERRUST_DESKTOP_HOME), Anmeldung admin", (await api()) === 200);
  await waitFor("Suche nach dem Update", () => state().last_check_ms > tInstall, 120000);
  check("nach dem Update: kein weiteres Update angeboten", !fs.existsSync(pendingFile) && !state().last_error);
  await connect();
  check("Update-Fenster öffnet nicht erneut", !pages().some((p) => /update\.html/.test(p.url())));
  const m2 = await pageWith(/127\.0\.0\.1/);
  await m2.goto("http://tauri.localhost/update.html").catch(() => {});
  try {
    const u2 = await pageWith(/update\.html/, 10000);
    await u2.waitForSelector("#idle:not([hidden])", { timeout: 10000 });
    await sleep(1200);
    const cur = await u2.textContent("#current");
    check("Update-Seite: installierte Version " + UPD, cur.includes(UPD), { cur });
    await shot(u2, "04-nach-update");
  } finally {
    try { await browser.close(); } catch {}
  }

  // 8) quit + uninstall
  r = spawnSync(path.join(installDir, "paperrust-desktop.exe"), ["stop"], { env: env(), encoding: "utf8", timeout: 120000 });
  await waitFor("App beendet", () => appPids().length === 0, 90000);
  await sleep(1500);
  check("stop: keine Prozesse übrig", procs().length === 0, { left: procs(), cli: r.status });
  r = spawnSync(path.join(installDir, "uninstall.exe"), ["/S"], { timeout: 120000 });
  await waitFor("Deinstallation", () => reg(UNINST_KEY, "DisplayVersion") === null && !fs.existsSync(exe), 120000, 1000);
  check("deinstalliert, Datenordner bleibt", fs.existsSync(path.join(home, "pgdata", "PG_VERSION")));
}

function cleanup() {
  for (const l of procs()) {
    const pid = Number(l.split(" ")[0]);
    step("Restprozess beenden", { l });
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"]);
  }
  if (reg(UNINST_KEY, "DisplayVersion") !== null && fs.existsSync(path.join(installDir, "uninstall.exe"))) {
    spawnSync(path.join(installDir, "uninstall.exe"), ["/S"], { timeout: 120000 });
  }
  // The silent uninstall (Tauri template) leaves the product key (folder, language) in place.
  const productKey = `HKCU\\Software\\PaperRust\\${PRODUCT}`;
  if (spawnSync("reg", ["query", productKey]).status === 0) {
    spawnSync("reg", ["delete", productKey, "/f"]);
    step("Registry-Produktschlüssel entfernt", { productKey });
  }
  // WebView2-Ordner der App-Kennung je Datenordner (main.rs identifier_suffix)
  const norm = home.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  const wv = path.join(process.env.LOCALAPPDATA, "ch.paperrust.desktop.h" + sha(Buffer.from(norm)).slice(0, 10));
  if (fs.existsSync(wv)) {
    fs.rmSync(wv, { recursive: true, force: true });
    step("WebView2-Ordner entfernt", { wv });
  }
  server.close();
}

main()
  .then(() => { cleanup(); save(true); console.log(`\nE2E bestanden: ${checks.length} Prüfungen, ${((Date.now() - t0) / 1000).toFixed(0)} s`); process.exit(0); })
  .catch((e) => { console.error(e); try { cleanup(); } catch (c) { console.error(c); } save(false, e); process.exit(1); });
