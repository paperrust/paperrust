// Comparison A (Django) ↔ B (PaperRust) of an E2E round and report report.md.
//
//   node compare.mjs --a <dirA> --b <dirB> --report <dir> [--rustlog <rust access log>] [--threshold 0.002]
//
// Per step: pixel diff (pixelmatch, color threshold 0.1; a step counts as deviating if the share of
// differing pixels > --threshold), visible text (lines only in A / only in B), table contents,
// network errors (4xx/5xx), console errors, step errors. Images: <report>/A, <report>/B, <report>/diff.
import fs from "node:fs";
import path from "node:path";
import { PNG } from "pngjs";
import pixelmatch from "pixelmatch";

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith("--") ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const THRESH = Number(args.threshold ?? 0.002);
const REP = args.report;
for (const d of ["A", "B", "diff"]) fs.mkdirSync(path.join(REP, d), { recursive: true });
const A = JSON.parse(fs.readFileSync(path.join(args.a, "steps.json"), "utf8"));
const B = JSON.parse(fs.readFileSync(path.join(args.b, "steps.json"), "utf8"));

// Rust access log: which path went via rust/django, and which route ID belongs to it
const viaByReq = new Map();
const rustRoutes = [];
if (args.rustlog && fs.existsSync(args.rustlog)) {
  const strip = (s) => s.replace(/\x1b\[[0-9;]*m/g, "");
  for (const raw of fs.readFileSync(args.rustlog, "utf8").split("\n")) {
    const l = strip(raw);
    let m = l.match(/rust route active id="([^"]+)" method=(\w+) path="([^"]+)"/);
    if (m) {
      const re = new RegExp("^" + m[3].replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\\\{[^}]+\\\}|\{[^}]+\}/g, "[^/]+") + "$");
      rustRoutes.push({ id: m[1], method: m[2], re, params: (m[3].match(/{/g) || []).length });
      rustRoutes.sort((a, b) => a.params - b.params); // fixed paths before {id} patterns
      continue;
    }
    m = l.match(/method=(\w+) path=(\S+) status=(\d+) ms=\d+ via="(\w+)"/);
    if (m) viaByReq.set(`${m[1]} ${m[2]}`, m[4]);
  }
}
function routeOf(method, url) {
  const p = url.split("?")[0];
  const r = rustRoutes.find((x) => x.method === method && x.re.test(p));
  const via = viaByReq.get(`${method} ${p}`) || (r ? "rust" : "django");
  return r ? `${via}:${r.id}` : via;
}

// Normalization (equal on both sides): times, 40-digit hex tokens, toast countdown
const norm = (s) => s.replace(/\b\d+ seconds?\b/g, "<n> seconds").replace(/\b[0-9a-f]{40}\b/g, "<token>").replace(/\b\d{1,2}:\d{2}(:\d{2})?( ?[AP]M)?\b/g, "<zeit>");
function lineDiff(a, b) {
  const cnt = new Map();
  for (const l of a.map(norm)) cnt.set(l, (cnt.get(l) || 0) + 1);
  const onlyB = [];
  for (const l of b.map(norm)) {
    if (cnt.get(l)) cnt.set(l, cnt.get(l) - 1);
    else onlyB.push(l);
  }
  const onlyA = [];
  for (const [l, c] of cnt) for (let i = 0; i < c; i++) onlyA.push(l);
  return { onlyA, onlyB };
}
const errKey = (e) => `${e.method} ${e.url.split("?")[0].replace(/\/\d+\//g, "/{id}/")} ${e.status}`;

function pixelDiff(fa, fb, out) {
  if (!fs.existsSync(fa) || !fs.existsSync(fb)) return { ratio: 1, note: "Bild fehlt" };
  const ia = PNG.sync.read(fs.readFileSync(fa));
  const ib = PNG.sync.read(fs.readFileSync(fb));
  const w = Math.max(ia.width, ib.width), h = Math.max(ia.height, ib.height);
  const pad = (img) => {
    if (img.width === w && img.height === h) return img;
    const p = new PNG({ width: w, height: h, fill: true });
    p.data.fill(255);
    PNG.bitblt(img, p, 0, 0, img.width, img.height, 0, 0);
    return p;
  };
  const pa = pad(ia), pb = pad(ib);
  const d = new PNG({ width: w, height: h });
  const px = pixelmatch(pa.data, pb.data, d.data, w, h, { threshold: 0.1, includeAA: false, alpha: 0.3 });
  const ratio = px / (w * h);
  if (ratio > THRESH) fs.writeFileSync(out, PNG.sync.write(d));
  return { ratio, px, size: `${ia.width}x${ia.height}` + (ia.width !== ib.width || ia.height !== ib.height ? ` vs ${ib.width}x${ib.height}` : "") };
}

const rows = [];
const details = [];
const byId = new Map(B.steps.map((s) => [s.id, s]));
for (const sa of A.steps) {
  const sb = byId.get(sa.id);
  const png = sa.id + ".png";
  fs.copyFileSync(path.join(args.a, png), path.join(REP, "A", png));
  if (sb && fs.existsSync(path.join(args.b, png))) fs.copyFileSync(path.join(args.b, png), path.join(REP, "B", png));
  if (!sb) { rows.push({ id: sa.id, status: "fehlt in B" }); continue; }
  const pd = pixelDiff(path.join(args.a, png), path.join(args.b, png), path.join(REP, "diff", png));
  const td = lineDiff(sa.text, sb.text);
  const tablesEqual = JSON.stringify(sa.tables.map((t) => t.map((r) => r.map(norm)))) === JSON.stringify(sb.tables.map((t) => t.map((r) => r.map(norm))));
  const neA = new Set(sa.netErrors.map(errKey)), neB = new Set(sb.netErrors.map(errKey));
  const netOnlyA = [...neA].filter((k) => !neB.has(k)), netOnlyB = [...neB].filter((k) => !neA.has(k));
  const ceA = new Set(sa.consoleErrors.map(norm)), ceB = new Set(sb.consoleErrors.map(norm));
  const conOnlyA = [...ceA].filter((k) => !ceB.has(k)), conOnlyB = [...ceB].filter((k) => !ceA.has(k));
  const toastDiff = lineDiff(sa.toasts, sb.toasts);
  const urlDiff = sa.url !== sb.url;
  const errDiff = (sa.error || "") !== (sb.error || "") || (sa.skipped || "") !== (sb.skipped || "");
  const pixel = pd.ratio > THRESH;
  const text = td.onlyA.length + td.onlyB.length > 0;
  const differs = pixel || text || !tablesEqual || netOnlyA.length || netOnlyB.length || conOnlyA.length || conOnlyB.length || toastDiff.onlyA.length || toastDiff.onlyB.length || urlDiff || errDiff;
  const kinds = [pixel && "Pixel", text && "Text", !tablesEqual && "Tabelle", (netOnlyA.length || netOnlyB.length) && "Netz", (conOnlyA.length || conOnlyB.length) && "Konsole", (toastDiff.onlyA.length || toastDiff.onlyB.length) && "Toast", urlDiff && "URL", errDiff && "Schritt"].filter(Boolean);
  rows.push({ id: sa.id, status: differs ? "**A≠B**" : "gleich", kinds: kinds.join(", "), ratio: pd.ratio, msA: sa.ms, msB: sb.ms, errA: (sa.serverDownBefore ? `[Server vor dem Schritt ${sa.serverDownBefore} s weg, neu gestartet] ` : "") + (sa.error || sa.skipped || ""), errB: (sb.serverDownBefore ? `[Server vor dem Schritt ${sb.serverDownBefore} s weg, neu gestartet] ` : "") + (sb.error || sb.skipped || ""), netA: sa.netErrors.length, netB: sb.netErrors.length });
  if (differs) {
    const bApiRust = (sb.api || []).map((l) => { const [m, u, st] = l.split(" "); return `${m} ${u.split("?")[0]} ${st} → ${routeOf(m, u)}`; });
    const bNet = sb.netErrors.map((e) => `${errKey(e)} → ${routeOf(e.method, e.url)}`);
    details.push({ sa, sb, pd, td, tablesEqual, netOnlyA, netOnlyB, conOnlyA, conOnlyB, toastDiff, urlDiff, kinds, bApiRust, bNet, png });
  }
}

// ---------- Bericht ----------
const md = [];
const fmt = (x) => (x * 100).toFixed(2) + " %";
const esc = (s) => String(s).replace(/\|/g, "\\|").replace(/\n/g, " ");
md.push(`# Browser-E2E A (Django) ↔ B (PaperRust)`, "");
md.push(`- A: ${A.base}  ·  B: ${B.base}  ·  Ablauf: ${A.flow}`);
md.push(`- Schritte: ${rows.length}, davon gleich: ${rows.filter((r) => r.status === "gleich").length}, abweichend: ${rows.filter((r) => r.status !== "gleich").length}`);
md.push(`- Pixel-Schwelle: ${fmt(THRESH)} abweichende Pixel (pixelmatch threshold 0.1, ohne Antialiasing); Masken: Toasts/Popover/Tooltips ausgeblendet, Notiz-Zeitstempel und API-Token magenta überdeckt.`);
md.push(`- Text-Normalisierung (beidseitig): Uhrzeiten → <zeit>, 40-stellige Hex-Tokens → <token>, Toast-Countdown „N seconds“ → „<n> seconds“.`, "");
md.push(`| Schritt | Ergebnis | Art | Pixel-Diff | ms A | ms B | Netzfehler A/B | Schrittfehler A | Schrittfehler B |`);
md.push(`|---|---|---|---|---|---|---|---|---|`);
for (const r of rows) md.push(`| ${r.id} | ${r.status} | ${r.kinds || ""} | ${r.ratio !== undefined ? fmt(r.ratio) : ""} | ${r.msA ?? ""} | ${r.msB ?? ""} | ${r.netA ?? ""}/${r.netB ?? ""} | ${esc(r.errA || "")} | ${esc(r.errB || "")} |`);
md.push("");
md.push(`## Abweichungen im Detail`, "");
for (const d of details) {
  md.push(`### ${d.sa.id} — ${d.kinds.join(", ")}`, "");
  md.push(`- URL A: \`${d.sa.url}\`  ·  B: \`${d.sb.url}\``);
  if (d.sa.error || d.sb.error) md.push(`- Schrittfehler A: ${d.sa.error || "–"}  ·  B: ${d.sb.error || "–"}`);
  md.push(`- Pixel: ${fmt(d.pd.ratio)} (${d.pd.size || d.pd.note})`);
  if (d.td.onlyA.length) md.push(`- Text nur in A: ${d.td.onlyA.slice(0, 15).map((l) => "`" + l.slice(0, 120) + "`").join(", ")}${d.td.onlyA.length > 15 ? ` … (+${d.td.onlyA.length - 15})` : ""}`);
  if (d.td.onlyB.length) md.push(`- Text nur in B: ${d.td.onlyB.slice(0, 15).map((l) => "`" + l.slice(0, 120) + "`").join(", ")}${d.td.onlyB.length > 15 ? ` … (+${d.td.onlyB.length - 15})` : ""}`);
  if (!d.tablesEqual) md.push(`- Tabelleninhalte unterscheiden sich`);
  if (d.toastDiff.onlyA.length || d.toastDiff.onlyB.length) md.push(`- Toast nur A: ${d.toastDiff.onlyA.join(" / ") || "–"}  ·  nur B: ${d.toastDiff.onlyB.join(" / ") || "–"}`);
  if (d.netOnlyA.length) md.push(`- Netzfehler nur A: ${d.netOnlyA.join(", ")}`);
  if (d.netOnlyB.length) md.push(`- Netzfehler nur B: ${d.netOnlyB.join(", ")}`);
  if (d.bNet.length) md.push(`- Netzfehler B (Zuordnung): ${d.bNet.join("; ")}`);
  if (d.conOnlyA.length) md.push(`- Konsole nur A: ${d.conOnlyA.map((c) => "`" + c.slice(0, 200) + "`").join(", ")}`);
  if (d.conOnlyB.length) md.push(`- Konsole nur B: ${d.conOnlyB.map((c) => "`" + c.slice(0, 200) + "`").join(", ")}`);
  const rustCalls = d.bApiRust.filter((l) => l.includes("rust:"));
  if (rustCalls.length) md.push(`- API-Aufrufe von B über Rust in diesem Schritt: ${[...new Set(rustCalls)].slice(0, 20).map((l) => "`" + l + "`").join(", ")}`);
  md.push("");
  const hasDiff = fs.existsSync(path.join(REP, "diff", d.png));
  md.push(`| A (Django) | B (PaperRust) | Diff |`, `|---|---|---|`);
  md.push(`| ![A](A/${d.png}) | ![B](B/${d.png}) | ${hasDiff ? `![diff](diff/${d.png})` : "–"} |`, "");
}
// Console/network errors that occur identically on both sides (background, not A≠B)
const both = new Map();
for (const s of A.steps) for (const e of s.netErrors) both.set(errKey(e), (both.get(errKey(e)) || 0) + 1);
md.push(`## Netzfehler A gesamt (Referenzverhalten, zum Abgleich)`, "");
md.push(both.size ? [...both].map(([k, c]) => `- ${k} (${c}×)`).join("\n") : "- keine", "");
fs.writeFileSync(path.join(REP, "report.md"), md.join("\n"));
fs.writeFileSync(path.join(REP, "summary.json"), JSON.stringify(rows, null, 1));
console.log(`Bericht: ${path.join(REP, "report.md")}  (${rows.filter((r) => r.status !== "gleich").length} von ${rows.length} Schritten abweichend)`);
