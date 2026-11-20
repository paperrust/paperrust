// Smoke test for desktop settings (remaining item 3): fresh PAPERRUST_DESKTOP_HOME, debug programs in
// package layout. First start (account from the environment) → change desktop.server.port via API v2 →
// supervisor writes desktop.toml and restarts the server → app reachable on the new port.
//   node smoke.mjs <pkg> <home> <out>
import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

const [pkg, home, out] = process.argv.slice(2).map((p) => path.resolve(p));
fs.mkdirSync(out, { recursive: true });
const log = [];
const t0 = Date.now();
const step = (name, data = {}) => {
  const e = { t: ((Date.now() - t0) / 1000).toFixed(1), step: name, ...data };
  log.push(e);
  console.log(`[${e.t}s] ${name}`, JSON.stringify(data));
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PASS = "smoke-" + Date.now().toString(36);
const env = { ...process.env };
for (const k of Object.keys(env)) if (/^(PAPERRUST_|PAPERLESS_|PG)/.test(k)) delete env[k];
Object.assign(env, { PAPERRUST_DESKTOP_HOME: home, PAPERRUST_DESKTOP_ADMIN_USER: "admin", PAPERRUST_DESKTOP_ADMIN_PASSWORD: PASS,
  PAPERRUST_DESKTOP_ADMIN_EMAIL: "admin@example.org" });
const desktop = path.join(pkg, "paperrust-desktop.exe");
const outLog = fs.openSync(path.join(out, "supervisor.log"), "w");
const sup = spawn(desktop, ["run", "--no-browser"], { env, stdio: ["pipe", outLog, outLog] });
step("supervisor gestartet", { pid: sup.pid });

const portFromToml = () => {
  try {
    const t = fs.readFileSync(path.join(home, "desktop.toml"), "utf8");
    const m = t.match(/\[server\][^\[]*?\bport\s*=\s*(\d+)/s);
    return m ? +m[1] : null;
  } catch { return null; }
};
async function up(port) {
  try {
    const r = await fetch(`http://127.0.0.1:${port}/api/`, { redirect: "manual", signal: AbortSignal.timeout(3000) });
    return r.status;
  } catch { return 0; }
}
async function waitFor(fn, ms, what) {
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const v = await fn();
    if (v) return v;
    await sleep(500);
  }
  throw new Error("timeout: " + what);
}
const freePort = () => new Promise((res) => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => res(p)); }); });

let ok = false;
try {
  // Port from the supervisor message (desktop.toml initially has the default 58080, which can be taken
  // on this machine by another installation)
  const started = () => [...fs.readFileSync(path.join(out, "supervisor.log"), "utf8").matchAll(/PaperRust-Server gestartet.*?port=(\d+)/g)].map((m) => +m[1]);
  const port1 = await waitFor(async () => { const p = started()[0]; return p && (await up(p)) ? p : null; }, 300000, "first start");
  step("erst-start fertig, server erreichbar", { port: port1, status: await up(port1) });
  const tok = await (await fetch(`http://127.0.0.1:${port1}/rs/api/v2/auth/devices`, { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username: "admin", password: PASS, device_name: "smoke" }) })).json();
  if (!tok.token) throw new Error("no device token: " + JSON.stringify(tok));
  const H = { Authorization: `Bearer ${tok.token}`, Accept: "application/json", "Content-Type": "application/json" };
  const cur = await (await fetch(`http://127.0.0.1:${port1}/rs/api/v2/settings/desktop.server.port`, { headers: H })).json();
  step("einstellung gelesen", { key: cur.key, value: cur.value, source: cur.source, restart: cur.restart });
  const port2 = await freePort();
  const r = await fetch(`http://127.0.0.1:${port1}/rs/api/v2/settings`, { method: "PATCH", headers: H, body: JSON.stringify({ values: { "desktop.server.port": String(port2) } }) });
  const body = await r.json().catch(() => null);
  step("PATCH desktop.server.port", { status: r.status, to: port2, body });
  if (r.status !== 202 && r.status !== 200) throw new Error("patch failed");
  const tRestart = Date.now();
  await waitFor(async () => started().includes(port2) && (await up(port2)) !== 0, 180000, "server on new port");
  step("neuer port erreichbar", { port: port2, status: await up(port2), seconds: (Date.now() - tRestart) / 1000, old_port_status: await up(port1),
    toml_port: portFromToml() });
  if (portFromToml() !== port2) throw new Error("desktop.toml not updated");
  const login = await fetch(`http://127.0.0.1:${port2}/rs/api/v2/me`, { headers: H });
  step("token auf neuem port", { status: login.status });
  const values = JSON.parse(fs.readFileSync(path.join(home, "run", "desktop-settings.json"), "utf8"));
  step("desktop-settings.json", { "desktop.server.port": values["desktop.server.port"] ?? values.values?.["desktop.server.port"] });
  ok = (await up(port1)) === 0 && login.status === 200;
} catch (e) {
  step("FEHLER", { error: String(e && e.stack || e) });
} finally {
  try { execFileSync(desktop, ["stop"], { env, timeout: 60000 }); step("stop"); } catch (e) { step("stop fehlgeschlagen", { error: String(e) }); }
  await sleep(2000);
  try { execFileSync("taskkill", ["/PID", String(sup.pid), "/T", "/F"], { stdio: "ignore" }); } catch {}
  fs.writeFileSync(path.join(out, "smoke-log.json"), JSON.stringify({ ok, steps: log }, null, 2));
  process.exit(ok ? 0 : 1);
}
