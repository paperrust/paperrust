#!/usr/bin/env node
// Generates src/components/common/bootstrap-icon/icons.generated.ts with exactly the icons that the
// Light UI uses (icon('name') / name="…" in src/), as inline SVG like <pngx-bs-icon>.
// Every icon must exist as a file in the npm package bootstrap-icons (node_modules/bootstrap-icons/icons/<name>.svg).
// Dynamic names (e.g. icon of a saved view) are in DYNAMIC.
//
// Budget first call (tools/budget.mjs): icons.generated.ts (core, statically loaded) contains only icons from
// files that are statically imported from the entry (src/main.ts) and the document list, plus DYNAMIC and
// every icon('…') outside `{@html …}` (there the text is adopted immediately). All remaining icons
// are in icons-extra.generated.ts; icon() loads them on demand (main.ts additionally when idle) and
// the markup redraws.
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const SRC = path.join(ROOT, 'src')
const OUT = path.join(SRC, 'components/common/bootstrap-icon/icons.generated.ts')
const OUT_EXTRA = path.join(SRC, 'components/common/bootstrap-icon/icons-extra.generated.ts')
/** Entries of the first call (like ROUTE_CHUNKS in tools/budget.mjs) */
const ENTRIES = ['main.ts', 'components/document-list/DocumentList.svelte']

// Icons whose name is only known at runtime
const DYNAMIC = [
  'funnel', 'plus-circle', 'dash-circle', 'chevron-double-right', 'chevron-double-left',
  'tag-fill', 'person-fill', 'file-earmark-fill', 'folder-fill', 'ui-radios', 'calendar-event-fill', 'person-fill-lock',
  // Upload status (open/closed), icons of saved views (test data: inbox, cash)
  'chevron-up', 'chevron-down', 'inbox', 'cash',
]

function walk(dir, files = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) walk(p, files)
    else if (/\.(svelte|ts)$/.test(e.name) && !e.name.endsWith('.generated.ts') && !e.name.endsWith('.test.ts')) files.push(p)
  }
  return files
}

const used = new Set(DYNAMIC)
const core = new Set(DYNAMIC)
for (const f of walk(SRC)) {
  const s = fs.readFileSync(f, 'utf8')
  for (const m of s.matchAll(/(\{@html\s+)?\bicon\(\s*'([a-z0-9-]+)'/g)) {
    used.add(m[2])
    // not in the markup ({@html …}): result is adopted immediately → must be there without lazy loading
    if (!m[1]) core.add(m[2])
  }
}

// Static import graph from the entries (without `import type` and without dynamic import())
function resolveImport(from, spec) {
  if (!spec.startsWith('.')) return null
  const base = path.resolve(path.dirname(from), spec)
  for (const c of [base, `${base}.ts`, `${base}.svelte`, `${base}.svelte.ts`, path.join(base, 'index.ts')]) {
    if (fs.existsSync(c) && fs.statSync(c).isFile()) return c
  }
  return null
}
const reachable = new Set()
const queue = ENTRIES.map((e) => path.join(SRC, e))
while (queue.length) {
  const f = queue.shift()
  if (reachable.has(f)) continue
  reachable.add(f)
  const s = fs.readFileSync(f, 'utf8')
  for (const m of s.matchAll(/(?:^|[\s;])(?:import|export)\s+(type\s+)?(?:[^'"()]*?\s+from\s+)?['"]([^'"]+)['"]/g)) {
    if (m[1]) continue
    const r = resolveImport(f, m[2])
    if (r && !r.endsWith('.generated.ts')) queue.push(r)
  }
}
for (const f of reachable) {
  if (!/\.(svelte|ts)$/.test(f)) continue
  for (const m of fs.readFileSync(f, 'utf8').matchAll(/\bicon\(\s*'([a-z0-9-]+)'/g)) core.add(m[1])
}
const ICONS = path.join(ROOT, 'node_modules/bootstrap-icons/icons')
// Name → file; short forms without hyphen before digits are allowed (dice5 → dice-5.svg)
const fileOf = (n) => [n, n.replace(/([a-z])(\d)/g, '$1-$2')].find((f) => fs.existsSync(path.join(ICONS, `${f}.svg`)))
const missing = [...used].filter((n) => !fileOf(n))
if (missing.length) {
  console.error(`[icons] nicht in bootstrap-icons: ${missing.join(', ')}`)
  process.exit(1)
}
const line = (n) => `  ${JSON.stringify(n)}: ${JSON.stringify(fs.readFileSync(path.join(ICONS, `${fileOf(n)}.svg`), 'utf8'))},`
const names = [...used].filter((n) => core.has(n)).sort()
const extra = [...used].filter((n) => !core.has(n)).sort()
fs.writeFileSync(
  OUT,
  `// ERZEUGT von tools/icons.mjs – nicht bearbeiten. ${names.length} Symbole (bootstrap-icons), Kern des ersten Aufrufs.\nexport const icons: Record<string, string> = {\n${names.map(line).join('\n')}\n}\n`,
)
fs.writeFileSync(
  OUT_EXTRA,
  `// ERZEUGT von tools/icons.mjs – nicht bearbeiten. ${extra.length} Symbole (bootstrap-icons), bei Bedarf nachgeladen.\nexport const extraIcons: Record<string, string> = {\n${extra.map(line).join('\n')}\n}\n`,
)
console.log(`[icons] ${names.length} + ${extra.length} Symbole → ${path.relative(ROOT, OUT)}, ${path.relative(ROOT, OUT_EXTRA)}`)
