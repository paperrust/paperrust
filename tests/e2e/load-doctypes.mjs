// Small load script (diagnosis of proxy hangs, E2E 2026-11-08): POST/PATCH/DELETE on
// /api/document_types/ via PaperRust (B) – these methods go through the proxy to Django –,
// in parallel to GET lists (Rust). Timing per phase: connection, response head, end.
//
//   node load-doctypes.mjs --base http://127.0.0.1:8171 --rounds 30 --conc 3 [--token <api-token>] [--out f.json]
//
// Local hosts only. Default token: admin token from reference/users.json (like warm() in run.sh).
import http from "node:http";
import fs from "node:fs";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith("--") ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const BASE = new URL(args.base || "http://127.0.0.1:8171");
if (!["127.0.0.1", "localhost", "::1"].includes(BASE.hostname)) throw new Error("nur lokale Hosts");
const ROUNDS = Number(args.rounds || 30);
const CONC = Number(args.conc || 3);
const TOKEN = args.token || "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const TIMEOUT = Number(args.timeout || 30000);
const agent = new http.Agent({ keepAlive: true, maxSockets: 6 });

function call(method, path, body) {
  return new Promise((resolve) => {
    const t0 = performance.now();
    const rec = { method, path, at: new Date().toISOString() };
    const data = body ? Buffer.from(JSON.stringify(body)) : null;
    const req = http.request({
      host: BASE.hostname, port: BASE.port, method, path, agent,
      headers: { Authorization: "Token " + TOKEN, Accept: "application/json; version=9", ...(data ? { "Content-Type": "application/json", "Content-Length": data.length } : {}) },
    });
    req.setTimeout(TIMEOUT, () => { rec.error = "timeout"; req.destroy(); });
    req.on("socket", (s) => {
      if (s.connecting) s.once("connect", () => { rec.connect_ms = Math.round(performance.now() - t0); });
      else rec.connect_ms = 0;
    });
    req.on("response", (res) => {
      rec.status = res.statusCode;
      rec.head_ms = Math.round(performance.now() - t0);
      let buf = "";
      res.on("data", (c) => (buf += c));
      res.on("end", () => { rec.total_ms = Math.round(performance.now() - t0); try { rec.json = JSON.parse(buf); } catch {} resolve(rec); });
    });
    req.on("error", (e) => { rec.error = rec.error || e.message; rec.total_ms = Math.round(performance.now() - t0); resolve(rec); });
    if (data) req.write(data);
    req.end();
  });
}

const all = [];
async function worker(w) {
  for (let r = 1; r <= ROUNDS; r++) {
    const name = `Last w${w} r${r} ${Date.now()}`;
    const c = await call("POST", "/api/document_types/", { name, matching_algorithm: 6, match: "", is_insensitive: true });
    all.push(c);
    const id = c.json?.id;
    all.push(await call("GET", "/api/document_types/?page_size=100"));
    if (id) {
      all.push(await call("PATCH", `/api/document_types/${id}/`, { name: name + " neu" }));
      all.push(await call("DELETE", `/api/document_types/${id}/`));
    }
  }
}
const t0 = Date.now();
await Promise.all(Array.from({ length: CONC }, (_, i) => worker(i + 1)));
for (const r of all) delete r.json;
const bad = all.filter((r) => r.error || !(r.status >= 200 && r.status < 300));
const by = {};
for (const r of all) (by[r.method] ||= []).push(r.head_ms ?? Infinity);
const q = (a, p) => { const s = [...a].sort((x, y) => x - y); return s[Math.min(s.length - 1, Math.floor(p * s.length))]; };
const summary = {
  requests: all.length, failed: bad.length, secs: Math.round((Date.now() - t0) / 1000),
  head_ms: Object.fromEntries(Object.entries(by).map(([m, a]) => [m, { n: a.length, p50: q(a, 0.5), p95: q(a, 0.95), max: Math.max(...a) }])),
  over_15s: all.filter((r) => (r.head_ms ?? Infinity) > 15000).length,
  failures: bad.slice(0, 20),
};
console.log(JSON.stringify(summary, null, 1));
if (args.out) fs.writeFileSync(args.out, JSON.stringify({ summary, all }, null, 1));
agent.destroy();
