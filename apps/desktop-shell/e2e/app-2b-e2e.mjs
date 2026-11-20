// PaperRust Desktop – mode 2b/3 end-to-end (built app, WebView2 CDP): app UI
// (light UI via prapi), own login page, selection + reconciliation, then server stopped → list,
// search and PDF from the replica, server back → online. In addition (remaining item 2b): links into the
// classic UI (online → system browser, here only logged via
// PAPERRUST_DESKTOP_E2E_OPEN_LOG; offline → "nur online verfügbar") and `/ws/status/` via the
// core's relay (upload → status_update; offline gently rejected). Local test servers only.
//
//   node apps/desktop-shell/e2e/app-2b-e2e.mjs <app-folder> <home> <out> <server-url> <server-pid> <restart-cmd>
//
//   <app-folder>  contains paperrust-app.exe and light\ (light UI build)
//   <home>        fresh PAPERRUST_DESKTOP_HOME (app.toml is created here)
//   <server-pid>  PID of the local test server (killed by PID for the offline part)
//   <restart-cmd> shell command that starts the test server again and prints its new PID
//
// Playwright from tests/e2e/node_modules (like apps/desktop/e2e/desktop-e2e.mjs).
import { createRequire } from "node:module";
import { spawn, execSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..", "..");
const require = createRequire(path.join(repo, "tests", "e2e", "package.json"));
const { chromium } = require("playwright");

const [appDir, home, outDir, serverUrl, serverPidArg, restartCmd] = process.argv.slice(2);
if (!appDir || !home || !outDir || !serverUrl || !serverPidArg) {
  console.error("usage: node app-2b-e2e.mjs <app> <home> <out> <server-url> <server-pid> <restart-cmd>");
  process.exit(2);
}
if (!/^http:\/\/127\.0\.0\.1:\d+\/$/.test(serverUrl)) throw new Error("only local test servers (http://127.0.0.1:<port>/)");
fs.mkdirSync(outDir, { recursive: true });
fs.mkdirSync(home, { recursive: true });
const PROFILE = "p6app01";
// E2E_FLAVOR=ngx: counterpart is an unmodified paperless-ngx (P6), login E2E_USER/E2E_PASS
const FLAVOR = process.env.E2E_FLAVOR === "ngx" ? "ngx" : "paperrust";
const USER = process.env.E2E_USER || "admin";
const PASS = process.env.E2E_PASS || "admin";
const ngxUuid = (code, id) => `00000000-0000-4000-8${code.toString(16).padStart(3, "0")}-${id.toString(16).padStart(12, "0")}`;
const CDP = 9336;
const APP = "http://prapi.localhost";
const log = [];
const t0 = Date.now();
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function step(name, data = {}) {
  const e = { t: ((Date.now() - t0) / 1000).toFixed(1), step: name, ...data };
  log.push(e);
  console.log(`[${e.t}s] ${name}`, Object.keys(data).length ? JSON.stringify(data) : "");
}
function save(ok, error) {
  fs.writeFileSync(path.join(outDir, "app-2b-log.json"), JSON.stringify({ ok, error: error && String(error), steps: log }, null, 2));
}
async function shot(page, name) {
  await page.screenshot({ path: path.join(outDir, `${name}.png`) }).catch(() => {});
}

fs.writeFileSync(
  path.join(home, "app.toml"),
  `mode = "remote"\nactive_profile = "${PROFILE}"\n\n[[profile]]\nid = "${PROFILE}"\nname = "P6 Test"\nurl = "${serverUrl}"\n` +
    `flavor = "${FLAVOR}"\nui = "app"\ntls_pin = ""\nallow_http = true\nextra_origins = []\nsync = true\n`,
);

// fresh start: no token from an earlier run in the keychain
try { execFileSync("cmdkey", [`/delete:PaperRust/${PROFILE}`], { stdio: "ignore" }); } catch {}
const env = { ...process.env };
for (const k of Object.keys(env)) if (/^(PAPERRUST_|PAPERLESS_|PG)/.test(k)) delete env[k];
env.PAPERRUST_DESKTOP_HOME = home;
const OPEN_LOG = path.join(outDir, "open-links.log");
try { fs.rmSync(OPEN_LOG); } catch {}
env.PAPERRUST_DESKTOP_E2E_OPEN_LOG = OPEN_LOG;
env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = `--remote-debugging-port=${CDP}`;
// Tests do not query an update server (E2E_UPDATES=on switches the search back on)
env.PAPERRUST_DESKTOP_UPDATES = process.env.E2E_UPDATES || "off";
const app = spawn(path.join(appDir, "paperrust-app.exe"), [], { env, stdio: "ignore" });
step("app gestartet", { pid: app.pid });

async function connect() {
  for (let i = 0; i < 60; i++) {
    try {
      return await chromium.connectOverCDP(`http://127.0.0.1:${CDP}`);
    } catch {
      await sleep(500);
    }
  }
  throw new Error("CDP not reachable");
}

async function appPage(browser) {
  for (let i = 0; i < 120; i++) {
    for (const c of browser.contexts()) for (const p of c.pages()) if (p.url().startsWith(APP)) return p;
    await sleep(500);
  }
  throw new Error("main window never reached " + APP);
}

/** fetch in the page context (same origin as the light UI) */
async function call(page, method, url, body) {
  return page.evaluate(
    async ([method, url, body]) => {
      const r = await fetch(url, { method, headers: body ? { "Content-Type": "application/json", Accept: "application/json" } : { Accept: "application/json; version=9" }, body: body ? JSON.stringify(body) : undefined });
      const ct = r.headers.get("content-type") || "";
      let data = null;
      if (ct.includes("json")) data = await r.json().catch(() => null);
      else {
        const b = new Uint8Array(await r.arrayBuffer());
        data = { bytes: b.length, head: String.fromCharCode(...b.slice(0, 5)) };
      }
      return { status: r.status, source: r.headers.get("x-paperrust-source"), ct, data };
    },
    [method, url, body],
  );
}

async function waitStatus(page, pred, ms = 180000) {
  const until = Date.now() + ms;
  let st;
  while (Date.now() < until) {
    st = (await call(page, "GET", "/local/status")).data;
    if (pred(st)) return st;
    await sleep(1000);
  }
  throw new Error("status not reached: " + JSON.stringify(st && st.status));
}

/** WebSocket via the relay (address from /local/ws): collect messages until `pred` matches. */
async function wsProbe(page, ms, upload) {
  return page.evaluate(
    async ([ms, upload]) => {
      const info = await fetch("/local/ws", { headers: { Accept: "application/json" } }).then((r) => r.json());
      if (!info.url) return { info, opened: false, messages: [] };
      const messages = [];
      let opened = false;
      const ws = new WebSocket(info.url);
      const done = new Promise((resolve) => {
        const t = setTimeout(resolve, ms);
        ws.onopen = async () => {
          opened = true;
          if (upload) {
            const pdf = "%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj 2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj " +
              "3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n";
            const fd = new FormData();
            fd.append("document", new Blob([pdf], { type: "application/pdf" }), upload);
            const r = await fetch("/api/documents/post_document/", { method: "POST", body: fd, headers: { Accept: "application/json; version=9" } });
            messages.push({ upload: r.status });
          }
        };
        ws.onmessage = (ev) => {
          messages.push(JSON.parse(ev.data));
          if (upload && messages.some((m) => m.type === "status_update" && m.data && m.data.filename === upload)) {
            clearTimeout(t);
            resolve();
          }
        };
        ws.onclose = (ev) => {
          messages.push({ closed: ev.code });
          clearTimeout(t);
          resolve();
        };
      });
      await done;
      try { ws.close(); } catch {}
      return { info: { online: info.online, port: new URL(info.url).port }, opened, messages };
    },
    [ms, upload],
  );
}

let ok = false;
let serverPid = serverPidArg;
try {
  const browser = await connect();
  let page = await appPage(browser);
  // not logged in → own login page under prapi
  await page.waitForURL(/\/accounts\/login\//, { timeout: 60000 });
  step("anmeldeseite", { url: page.url() });
  await shot(page, "01-login");
  await page.fill("#u", USER);
  await page.fill("#p", PASS);
  await page.click("#go");
  await page.waitForURL(/\/rs\/app\//, { timeout: 60000 });
  await page.waitForSelector("pngx-app-frame", { state: "attached", timeout: 60000 });
  step("angemeldet, App geladen", { url: page.url() });
  await shot(page, "02-app-online");
  const kc = execSync(`cmdkey /list:PaperRust/${PROFILE}`).toString();
  step("schluesselbund", { entry: /PaperRust\/p6app01/.test(kc) });

  // Selection: PaperRust all visible documents; paperless-ngx tag "Invoices" (preferred files)
  let items;
  if (FLAVOR === "ngx") {
    const tags = await call(page, "GET", "/api/tags/?page=1&page_size=100000");
    const inv = tags.data.results.find((t) => t.name === "Invoices");
    items = [{ type: "tag", key: ngxUuid(2, inv.id), files: "preferred" }];
  } else {
    const ids = await call(page, "GET", "/rs/api/v2/sync/ids?kind=document");
    if (ids.status !== 200) throw new Error("ids " + ids.status);
    items = ids.data.items.map(([u]) => ({ type: "document", key: u, files: "preferred" }));
  }
  const plan = await call(page, "PUT", "/local/selection", { items });
  step("auswahl", { status: plan.status, documents: plan.data && plan.data.total_documents, bytes: plan.data && plan.data.total_bytes });
  const tSync = Date.now();
  const want = plan.data.total_documents;
  const tSel = Date.now() - 1000;
  // paperless-ngx: `all` can report IDs that the list does not return (versions) → only check "fertig"
  const st = await waitStatus(page, (s) => s.status.state === "idle" && s.status.phase === null && s.documents > 0 &&
    Date.parse(s.status.last_sync || 0) >= tSel && (FLAVOR === "ngx" || s.documents >= want));
  step("abgeglichen", { documents: st.documents, cache_bytes: st.cache_bytes, seconds: (Date.now() - tSync) / 1000 });

  // List online (server), source remote
  const onl = await call(page, "GET", "/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true");
  step("liste online", { status: onl.status, source: onl.source, count: onl.data && onl.data.count });
  await page.goto(`${APP}/rs/app/documents`);
  await page.waitForSelector("pngx-app-frame", { state: "attached", timeout: 60000 });
  await sleep(3000);
  await shot(page, "03-liste-online");

  // Link into the classic UI (online): window stays in the app, target on the server in the browser
  const before = page.url();
  await page.evaluate(() => { location.href = "/documents/1/details"; });
  await sleep(2500);
  const opened = fs.existsSync(OPEN_LOG) ? fs.readFileSync(OPEN_LOG, "utf8").trim().split(/\r?\n/) : [];
  step("klassisch online", { url: page.url(), opened });
  if (page.url() !== before || !opened.includes(`${serverUrl}documents/1/details`)) throw new Error("classic link (online) not opened externally");

  // /ws/status/ online: upload → status_update arrives via the relay (server: PAPERRUST_QUEUE=postgres)
  if (FLAVOR === "paperrust") {
    const csp = await page.evaluate(async () => (await fetch("/rs/app/")).headers.get("content-security-policy"));
    const name = `e2e-ws-${Date.now()}.pdf`;
    const w = await wsProbe(page, 90000, name);
    const hits = w.messages.filter((m) => m.type === "status_update" && m.data && m.data.filename === name);
    step("websocket online", { opened: w.opened, port: w.info.port, upload: (w.messages.find((m) => m.upload) || {}).upload,
      status_updates: hits.map((m) => `${m.data.status}:${m.data.message}`), csp_ws: /connect-src 'self' ws:\/\/127\.0\.0\.1:\d+/.test(csp || "") });
    if (!w.opened || hits.length === 0) throw new Error("no status_update over the ws relay");
  }

  // ---- stop server → offline
  // /T: Granian (paperless-ngx) has worker processes under the main process
  execFileSync("taskkill", ["/PID", String(serverPid), "/T", "/F"]);
  for (let i = 0; i < 40; i++) {
    const up = await fetch(serverUrl).then(() => true, () => false);
    if (!up) break;
    await sleep(500);
  }
  step("server gestoppt", { pid: serverPid });
  await sleep(500);
  const off = await call(page, "GET", "/api/documents/?page=1&page_size=25&ordering=-created&truncate_content=true&include_selection_data=true");
  step("liste offline", { status: off.status, source: off.source, count: off.data && off.data.count });
  if (off.status !== 200 || off.source !== "local" || !(off.data.count > 0)) throw new Error("offline list failed");
  await page.goto(`${APP}/rs/app/documents`);
  await page.waitForSelector("pngx-app-frame", { state: "attached", timeout: 60000 });
  await sleep(3000);
  const rows = await page.locator("table tbody tr, pngx-document-card-small, .document-card, .card").count();
  step("liste offline (UI)", { rows });
  await shot(page, "04-liste-offline");
  const search = await call(page, "GET", "/api/documents/?page=1&page_size=25&ordering=-created&title_content=invoice");
  step("suche offline", { status: search.status, source: search.source, count: search.data && search.data.count });
  if (!(search.data && search.data.count > 0)) throw new Error("offline search found nothing");
  await page.goto(`${APP}/rs/app/documents?title_content=invoice`);
  await sleep(3000);
  await shot(page, "05-suche-offline");
  // PDF: first document with a PDF
  const pdfDoc = off.data.results.find((d) => d.mime_type === "application/pdf") || off.data.results[0];
  const pdf = await call(page, "GET", `/api/documents/${pdfDoc.id}/preview/`);
  step("pdf offline", { id: pdfDoc.id, status: pdf.status, source: pdf.source, head: pdf.data && pdf.data.head, bytes: pdf.data && pdf.data.bytes });
  if (pdf.status !== 200 || !(pdf.data.head || "").startsWith("%PDF")) throw new Error("offline PDF failed");
  await page.goto(`${APP}/rs/app/documents/${pdfDoc.id}/details`);
  await sleep(5000);
  await shot(page, "06-dokument-offline");
  // Link into the classic UI (offline): notice page "Nur online verfügbar"
  await page.evaluate((id) => { location.href = `/documents/${id}/notes`; }, pdfDoc.id);
  await page.waitForURL(/\/documents\/\d+\/notes/, { timeout: 15000 });
  const h1 = await page.locator("h1").first().textContent().catch(() => "");
  step("klassisch offline", { url: page.url(), h1 });
  await shot(page, "07-klassisch-offline");
  if (!/Nur online verfügbar/.test(h1 || "")) throw new Error("offline classic page missing");
  await page.goto(`${APP}/rs/app/documents`);
  await page.waitForSelector("pngx-app-frame", { state: "attached", timeout: 60000 });
  if (FLAVOR === "paperrust") {
    const w = await wsProbe(page, 10000, null);
    step("websocket offline", { opened: w.opened, online: w.info.online, messages: w.messages });
    if (w.opened) throw new Error("ws relay accepted offline");
  }
  const write = await call(page, "POST", `/api/documents/${pdfDoc.id}/notes/`, { note: "offline" });
  step("schreiben offline", { status: write.status });

  // ---- start server again → online
  if (restartCmd) {
    serverPid = execSync(restartCmd, { shell: "bash" }).toString().trim().split(/\s+/).pop();
    step("server neu gestartet", { pid: serverPid });
    await call(page, "POST", "/local/sync");
    const st2 = await waitStatus(page, (s) => s.status.online && s.status.state === "idle", 120000);
    const back = await call(page, "GET", "/api/documents/?page=1&page_size=5");
    step("wieder online", { state: st2.status.state, source: back.source });
  }

  // Clean up: revoke device, delete keychain + replica
  const lo = await call(page, "POST", "/local/logout", { wipe: true });
  step("abgemeldet", { status: lo.status, revoked: lo.data && lo.data.revoked });
  await browser.close().catch(() => {});
  ok = true;
} catch (e) {
  step("FEHLER", { error: String(e && e.stack || e) });
  save(false, e);
} finally {
  try { execFileSync("taskkill", ["/PID", String(app.pid), "/T", "/F"]); } catch {}
  if (ok) save(true);
  console.log("server-pid", serverPid);
  process.exit(ok ? 0 : 1);
}
