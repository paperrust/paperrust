// Small, safe Markdown renderer for chat answers (stand-in for pipes/markdown.pipe.ts, which uses `marked` with
// gfm + breaks). Chat content is untrusted: every text piece is HTML-escaped, raw HTML and images stay text,
// links only with http/https/mailto (target=_blank, rel=noopener noreferrer nofollow, hostname appended like
// upstream). Supported: paragraphs with line breaks, headings, ordered/unordered lists, fenced code blocks (no
// indented ones), blockquotes, GFM tables, inline code, bold, italic, strikethrough, links.

const ALLOWED_LINK_PROTOCOLS = new Set(['http:', 'https:', 'mailto:'])

export function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;')
}

function allowedUrl(href: string): URL | null {
  try {
    const url = new URL(href)
    return ALLOWED_LINK_PROTOCOLS.has(url.protocol) ? url : null
  } catch {
    return null
  }
}

const INLINE = /`([^`\n]+)`|!?\[([^\]\n]*)\]\(([^)\s]+)(?:\s+"[^"\n]*")?\)|\*\*(?=\S)([\s\S]*?\S)\*\*|__(?=\S)([\s\S]*?\S)__|~~(?=\S)([\s\S]*?\S)~~|\*(?=[^\s*])([^*\n]*?[^\s*])\*/g

/** Inline Markdown → HTML (escaped) */
export function renderInline(raw: string): string {
  let out = ''
  let last = 0
  const re = new RegExp(INLINE.source, 'g')
  let m: RegExpExecArray | null
  while ((m = re.exec(raw))) {
    out += escapeHtml(raw.slice(last, m.index))
    last = m.index + m[0].length
    if (m[1] !== undefined) out += `<code>${escapeHtml(m[1])}</code>`
    else if (m[3] !== undefined) {
      if (m[0].startsWith('!')) {
        out += escapeHtml(m[0]) // images: plain text
        continue
      }
      const url = allowedUrl(m[3])
      const text = renderInline(m[2] ?? '')
      if (!url) out += text
      else {
        out += `<a target="_blank" rel="noopener noreferrer nofollow" href="${escapeHtml(m[3])}">${text}</a>`
        if (url.hostname) out += ` <span class="text-muted small">(${escapeHtml(url.hostname)})</span>`
      }
    } else if (m[4] !== undefined || m[5] !== undefined) out += `<strong>${renderInline(m[4] ?? m[5])}</strong>`
    else if (m[6] !== undefined) out += `<del>${renderInline(m[6])}</del>`
    else if (m[7] !== undefined) out += `<em>${renderInline(m[7])}</em>`
  }
  out += escapeHtml(raw.slice(last))
  return out
}

const UL = /^\s{0,3}[-*+]\s+(.*)$/
const OL = /^\s{0,3}(\d+)[.)]\s+(.*)$/
const HEADING = /^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$/
const FENCE = /^\s{0,3}(```|~~~)/
const QUOTE = /^\s{0,3}>\s?(.*)$/
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/
const HR = /^\s{0,3}([-*_])(\s*\1){2,}\s*$/

function splitRow(line: string): string[] {
  let s = line.trim()
  if (s.startsWith('|')) s = s.slice(1)
  if (s.endsWith('|')) s = s.slice(0, -1)
  return s.split('|').map((c) => c.trim())
}

/** Block Markdown → HTML */
export function renderMarkdown(value: string | null | undefined): string {
  if (!value) return ''
  const lines = value.replace(/\r\n?/g, '\n').split('\n')
  const out: string[] = []
  let i = 0
  const isBlockStart = (l: string) => UL.test(l) || OL.test(l) || HEADING.test(l) || FENCE.test(l) || QUOTE.test(l) || HR.test(l)
  while (i < lines.length) {
    const line = lines[i]
    if (!line.trim()) {
      i++
      continue
    }
    const fence = line.match(FENCE)
    if (fence) {
      const code: string[] = []
      i++
      while (i < lines.length && !lines[i].trimStart().startsWith(fence[1])) code.push(lines[i++])
      i++ // closing fence (or end)
      out.push(`<pre><code>${escapeHtml(code.join('\n'))}${code.length ? '\n' : ''}</code></pre>`)
      continue
    }
    const h = line.match(HEADING)
    if (h) {
      out.push(`<h${h[1].length}>${renderInline(h[2])}</h${h[1].length}>`)
      i++
      continue
    }
    if (HR.test(line)) {
      out.push('<hr>')
      i++
      continue
    }
    if (line.includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]) && lines[i + 1].includes('-')) {
      const head = splitRow(line)
      i += 2
      const rows: string[][] = []
      while (i < lines.length && lines[i].trim() && lines[i].includes('|')) rows.push(splitRow(lines[i++]))
      const th = head.map((c) => `<th>${renderInline(c)}</th>`).join('')
      const body = rows.map((r) => `<tr>${head.map((_, k) => `<td>${renderInline(r[k] ?? '')}</td>`).join('')}</tr>`).join('')
      out.push(`<table><thead><tr>${th}</tr></thead>${body ? `<tbody>${body}</tbody>` : ''}</table>`)
      continue
    }
    if (QUOTE.test(line)) {
      const inner: string[] = []
      while (i < lines.length && QUOTE.test(lines[i])) inner.push((lines[i++].match(QUOTE) as RegExpMatchArray)[1])
      out.push(`<blockquote>${renderMarkdown(inner.join('\n'))}</blockquote>`)
      continue
    }
    const ordered = OL.test(line)
    if (ordered || UL.test(line)) {
      const re = ordered ? OL : UL
      const items: string[] = []
      const start = ordered ? parseInt((line.match(OL) as RegExpMatchArray)[1], 10) : 1
      while (i < lines.length) {
        const mm = lines[i].match(re)
        if (mm) {
          items.push(ordered ? mm[2] : mm[1])
          i++
        } else if (lines[i].trim() && /^\s{2,}/.test(lines[i]) && items.length) {
          items[items.length - 1] += '\n' + lines[i].trim()
          i++
        } else break
      }
      const lis = items.map((it) => `<li>${it.split('\n').map(renderInline).join('<br>')}</li>`).join('')
      out.push(ordered ? `<ol${start !== 1 ? ` start="${start}"` : ''}>${lis}</ol>` : `<ul>${lis}</ul>`)
      continue
    }
    const para: string[] = []
    while (i < lines.length && lines[i].trim() && !(para.length && isBlockStart(lines[i]))) para.push(lines[i++])
    out.push(`<p>${para.map((l) => renderInline(l.trim())).join('<br>')}</p>`)
  }
  return out.join('\n')
}
