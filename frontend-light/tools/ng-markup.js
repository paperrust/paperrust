// Svelte preprocessor: whitespace rules like the Angular compiler (preserveWhitespaces: false),
// so that the markup adopted from the Angular templates wraps pixel-identically.
//   Angular: pure whitespace text nodes between elements/blocks are dropped entirely;
//            text with content keeps leading/trailing whitespace (collapsed to 1 space).
//   Svelte:  pure whitespace between elements → 1 space; edge whitespace in elements is dropped.
// Implementation: delete pure whitespace runs between tags/blocks, fix edge whitespace of text runs
// as {' '}. <script>/<style>/<pre>/<textarea> stay untouched.
// In addition: encapsulation attributes (tools/ng-scope.mjs) – sc-<name> on every element if the
// component imports styles/components/…/<name>.css, and sh-<x> on every host tag <pngx-x>.
import { addMarkupAttrs } from './ng-scope.mjs'

const SKIP = /^<(script|style|pre|textarea)\b/i

function scanBrace(s, i) {
  // s[i] === '{' → index after the matching '}' (strings/template literals taken into account)
  let depth = 0
  for (; i < s.length; i++) {
    const c = s[i]
    if (c === '"' || c === "'" || c === '`') {
      const q = c
      for (i++; i < s.length && s[i] !== q; i++) if (s[i] === '\\') i++
      continue
    }
    if (c === '{') depth++
    else if (c === '}' && --depth === 0) return i + 1
  }
  throw new Error('ng-whitespace: } fehlt')
}

function scanTag(s, i) {
  if (s.startsWith('<!--', i)) return s.indexOf('-->', i) + 3
  for (i++; i < s.length; i++) {
    const c = s[i]
    if (c === '"' || c === "'") {
      const q = c
      for (i++; i < s.length && s[i] !== q; i++) if (s[i] === '{') i = scanBrace(s, i) - 1
      continue
    }
    if (c === '{') {
      i = scanBrace(s, i) - 1
      continue
    }
    if (c === '>') return i + 1
  }
  throw new Error('ng-whitespace: > fehlt')
}

/** Splits markup into tokens: tag | block ({#…}{:…}{/…}{@const}) | expr ({…}, {@html}) | text */
export function tokenize(s) {
  const out = []
  let i = 0
  while (i < s.length) {
    const c = s[i]
    if (c === '<') {
      const m = SKIP.exec(s.slice(i, i + 12))
      let end
      if (m) {
        const close = s.toLowerCase().indexOf(`</${m[1].toLowerCase()}`, i)
        end = scanTag(s, close)
      } else end = scanTag(s, i)
      out.push({ k: 'tag', v: s.slice(i, end) })
      i = end
    } else if (c === '{') {
      const end = scanBrace(s, i)
      const v = s.slice(i, end)
      // {@render}/{@html} like <ng-content>/[innerHTML]: structural, whitespace around it is dropped
      out.push({ k: /^\{[#:/]|^\{@(const|render|html)\b/.test(v) ? 'block' : 'expr', v })
      i = end
    } else {
      let j = i
      while (j < s.length && s[j] !== '<' && s[j] !== '{') j++
      out.push({ k: 'text', v: s.slice(i, j) })
      i = j
    }
  }
  return out
}

export function transformMarkup(s, name = null) {
  const t = tokenize(s)
  const out = []
  let run = []
  const flush = () => {
    if (!run.length) return
    const content = run.some((x) => x.k === 'expr' || /\S/.test(x.v))
    if (content) {
      const first = run[0]
      const last = run[run.length - 1]
      if (first.k === 'text' && /^\s/.test(first.v)) first.v = "{' '}" + first.v.replace(/^\s+/, '')
      if (last.k === 'text' && /\s$/.test(last.v)) last.v = last.v.replace(/\s+$/, '') + "{' '}"
      for (const x of run) out.push(x.v)
    }
    run = []
  }
  for (const x of t) {
    if (x.k === 'tag' || x.k === 'block') {
      flush()
      out.push(x.k === 'tag' ? addMarkupAttrs(x.v, name) : x.v)
    } else run.push(x)
  }
  flush()
  return out.join('')
}

export function ngMarkup() {
  return {
    name: 'ng-markup',
    markup({ content, filename }) {
      if (filename && !filename.endsWith('.svelte')) return
      // Second UI (src/design2/): own design, normal Svelte rules (docs/DESIGN2.md)
      if (filename && /[\\/]src[\\/]design2[\\/]/.test(filename)) return
      const name = /styles\/components\/(?:[^'"]*\/)?([\w-]+)\.css['"]/.exec(content)?.[1] ?? null
      const parts = content.split(/(<script\b[\s\S]*?<\/script>|<style\b[\s\S]*?<\/style>)/)
      const code = parts.map((p, i) => (i % 2 ? p : transformMarkup(p, name))).join('')
      return { code }
    },
  }
}
