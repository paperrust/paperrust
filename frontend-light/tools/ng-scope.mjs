// Encapsulation of the component styles via attributes (instead of Svelte scoping via classes):
//   - every element in the markup of a component carries the attribute  sc-<name>
//   - every host element <pngx-foo> carries                             sh-foo
//   - selectors: every compound group gets [sc-<name>], :host → [sh-<name>],
//     :host-context(X) → "X[sh] …" and "X [sh] …", from ::deep on no more encapsulation.
// Own styles are under src/styles/components/<name>.css and are encapsulated at build time
// (Vite plugin scopedStyles in this module); tools/ng-markup.js sets the attributes in the markup.
// Svelte class scoping would, among other things, undermine Bootstrap's a:not([href]):not([class]) (class instead of attribute)
// and count the specificity differently (:where from the second occurrence).
import postcss from 'postcss'

const isCombinator = (t) => t === ' ' || t === '>' || t === '+' || t === '~'

/** Splits a complex selector into compound groups and combinators. */
export function splitCompounds(sel) {
  const out = []
  let cur = ''
  let depth = 0
  let quote = null
  const push = () => {
    if (cur) out.push(cur)
    cur = ''
  }
  for (let i = 0; i < sel.length; i++) {
    const c = sel[i]
    if (quote) {
      cur += c
      if (c === quote) quote = null
      continue
    }
    if (c === '"' || c === "'") {
      quote = c
      cur += c
      continue
    }
    if (c === '(' || c === '[') depth++
    if (c === ')' || c === ']') depth--
    if (depth === 0 && (c === ' ' || c === '>' || c === '+' || c === '~' || c === '\n' || c === '\t')) {
      push()
      const comb = c === '\n' || c === '\t' ? ' ' : c
      // collapse multiple spaces / " > "
      if (out.length && isCombinator(out[out.length - 1])) {
        if (comb !== ' ') out[out.length - 1] = comb
      } else if (out.length) out.push(comb)
      continue
    }
    cur += c
  }
  push()
  while (out.length && isCombinator(out[out.length - 1])) out.pop()
  return out
}

/** Insert attribute before the first pseudo-class/-element (outside of parentheses). */
function addAttr(compound, attr) {
  let depth = 0
  for (let i = 0; i < compound.length; i++) {
    const c = compound[i]
    if (c === '(' || c === '[') depth++
    else if (c === ')' || c === ']') depth--
    else if (c === ':' && depth === 0) return compound.slice(0, i) + attr + compound.slice(i)
  }
  return compound + attr
}

const join = (parts) =>
  parts
    .map((p) => (p === ' ' ? ' ' : isCombinator(p) ? ` ${p} ` : p))
    .join('')
    .replace(/\s+/g, ' ')
    .trim()

export function scopeSelector(sel, name) {
  const content = `[sc-${name}]`
  const host = `[sh-${name}]`
  const parts = splitCompounds(sel.trim())
  let variants = [[]]
  let deep = false
  for (const p of parts) {
    if (isCombinator(p)) {
      variants.forEach((v) => v.push(p))
      continue
    }
    if (deep) {
      variants.forEach((v) => v.push(p))
      continue
    }
    if (p === '::deep') {
      deep = true
      // "::deep" is its own group; a following combinator (space) is dropped
      variants.forEach((v) => {
        if (v.length && v[v.length - 1] === ' ') v.pop()
        v.push('\u0000DEEP')
      })
      continue
    }
    const hc = /^:host-context\((.+)\)(.*)$/.exec(p)
    if (hc) {
      const inner = hc[1]
      const rest = hc[2]
      const next = []
      for (const v of variants) {
        next.push([...v, `${inner}${host}${rest}`])
        next.push([...v, inner, ' ', `${host}${rest}`])
      }
      variants = next
      continue
    }
    const h = /^:host(?:\((.+)\))?(.*)$/.exec(p)
    if (h) {
      variants.forEach((v) => v.push(`${h[1] ?? ''}${host}${h[2] ?? ''}`))
      continue
    }
    variants.forEach((v) => v.push(addAttr(p, content)))
  }
  return variants.map((v) => {
    // DEEP marker: preceding combinator is kept, marker becomes a space or is dropped at the start
    const s = v
      .map((x, i) => (x === '\u0000DEEP' ? (i === 0 ? '' : ' ') : x))
      .filter((x, i, arr) => !(x === ' ' && (i === 0 || arr[i - 1] === ' ')))
    return join(s)
  })
}

/** Component CSS → encapsulated CSS. */
export function scopeCss(css, name) {
  const root = postcss.parse(css)
  root.walkRules((rule) => {
    const p = rule.parent
    if (p?.type === 'atrule' && /keyframes$/i.test(p.name)) return
    rule.selectors = rule.selectors.flatMap((s) => scopeSelector(s, name))
  })
  return root.toString()
}

// ---- Markup: set attributes (Svelte preprocessor) --------------------------------------------

/** Adds sc-<name> to every HTML element and sh-foo to every <pngx-foo>. */
export function addMarkupAttrs(tagToken, name) {
  // Components that render their host element themselves (NgSelect …): pass the encapsulation attribute through as a prop
  const c = /^<(Ng[A-Z][A-Za-z]*)(?=[\s/>])/.exec(tagToken)
  if (c && name) return `<${c[1]} sc-${name}=""${tagToken.slice(c[0].length)}`
  const m = /^<([a-z][a-z0-9-]*)(?=[\s/>])/.exec(tagToken)
  if (!m) return tagToken // closing tag, comment, component (uppercase letter), svelte:…
  const tag = m[1]
  if (tag === 'script' || tag === 'style') return tagToken
  let attrs = ''
  if (name) attrs += ` sc-${name}`
  if (tag.startsWith('pngx-')) attrs += ` sh-${tag.slice(5)}`
  return attrs ? `<${tag}${attrs}${tagToken.slice(m[0].length)}` : tagToken
}

// ---- Vite: encapsulate own component styles at build time ---------------------------------------------

/** Name of a component stylesheet (src/styles/components/…/<name>.css) or null. */
export function scopedStyleName(id) {
  return /[\\/]src[\\/]styles[\\/]components[\\/](?:[^?]*[\\/])?([\w-]+)\.css(?:\?|$)/.exec(id)?.[1] ?? null
}

/** Vite plugin: encapsulates src/styles/components/<name>.css with [sc-<name>]/[sh-<name>] before Vite processes the CSS. */
export function scopedStyles() {
  return {
    name: 'paperrust-scoped-styles',
    enforce: 'pre',
    transform(code, id) {
      const name = scopedStyleName(id)
      if (!name) return null
      return { code: scopeCss(code, name), map: null }
    },
  }
}
