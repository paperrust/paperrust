// Trim the Bootstrap CSS from the npm package (bootstrap, MIT) at build time to what the Light UI uses.
// Reason: the CSS budget of the classic UI (≤ 30 KB brotli, tools/budget.mjs); the full bootstrap.css
// alone has ~24 KB brotli. A rule stays if every class in its selector occurs somewhere in src/ (Svelte/TS/CSS)
// or in index.html/m.html as a word or is composed at runtime (DYNAMIC).
// Rules without classes (reboot, :root variables, elements) always stay.
import fs from 'node:fs'
import path from 'node:path'
import postcss from 'postcss'

// Classes that the code composes from parts (`bg-${color}`, `alert-${type}`, bs-popover-<side> …) or that
// set states (show, active …): never remove based on the word list.
export const DYNAMIC = /^(?:bg-|text-|text-bg-|btn-|alert-|border-|bs-popover-|bs-tooltip-|toast|progress|spinner|visually-hidden|fade|show|showing|hide|collapse|collapsing|active|disabled|is-|was-validated)/

/** All words (possible class names) from the sources of the Light UI. */
export function sourceWords(root) {
  const words = new Set()
  const add = (text) => {
    for (const w of text.match(/[A-Za-z0-9_-]+/g) ?? []) words.add(w)
  }
  const walk = (dir) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name)
      if (e.isDirectory()) walk(p)
      else if (/\.(svelte|ts|css|scss)$/.test(e.name) && !/\.test\.ts$/.test(e.name)) add(fs.readFileSync(p, 'utf8'))
    }
  }
  walk(path.join(root, 'src'))
  for (const f of ['index.html', 'm.html']) if (fs.existsSync(path.join(root, f))) add(fs.readFileSync(path.join(root, f), 'utf8'))
  return words
}

/** Removes selectors with classes that never occur; empty rules/at-rules are dropped. */
export function pruneCss(css, words) {
  // Classes in :not(…) only restrict (".btn:not(.btn-check)" also matches without .btn-check); never trim :is/:where
  const known = (sel) =>
    /:(?:is|where)\(/.test(sel) ||
    [...sel.replace(/:not\((?:[^()]|\([^()]*\))*\)/g, '').matchAll(/\.(-?[A-Za-z_][\w-]*)/g)].every((m) => words.has(m[1]) || DYNAMIC.test(m[1]))
  const root = postcss.parse(css)
  let removed = 0
  root.walkRules((rule) => {
    if (rule.parent?.type === 'atrule' && /keyframes$/i.test(rule.parent.name)) return
    const keep = rule.selectors.filter(known)
    if (keep.length === rule.selectors.length) return
    removed++
    if (keep.length) rule.selectors = keep
    else rule.remove()
  })
  let changed = true
  while (changed) {
    changed = false
    root.walkAtRules((at) => {
      if (at.nodes && at.nodes.length === 0) {
        at.remove()
        changed = true
      }
    })
  }
  root.walkComments((c) => c.remove())
  return { css: root.toString(), removed }
}

/** Vite plugin: `import 'bootstrap/dist/css/bootstrap.css'` delivers the trimmed version. */
export function bootstrapCss(root = process.cwd()) {
  return {
    name: 'paperrust-bootstrap-css',
    enforce: 'pre',
    transform(code, id) {
      if (!/[\\/]bootstrap[\\/]dist[\\/]css[\\/]bootstrap\.css(?:\?|$)/.test(id)) return null
      // sourceMappingURL points to a .map next to the package file; the trimmed version has none
      const { css } = pruneCss(code.replace(/\/\*# sourceMappingURL=.*\*\//, ''), sourceWords(root))
      return { code: css, map: null }
    },
  }
}
