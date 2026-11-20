// Builder for storage paths (feedback 2026-11-17: "Simplify creating folders"). A path consists of
// folder levels and the file name; each level is a placeholder of the upstream templates (Jinja, docs "storage
// paths"). Free templates remain possible in the "Advanced" field; only simple templates can be read back.

export type PartId = 'year' | 'month' | 'day' | 'correspondent' | 'document_type' | 'owner' | 'title' | 'asn'

export const PARTS: Record<PartId, { placeholder: string; example: string }> = {
  year: { placeholder: '{{ created_year }}', example: '2026' },
  month: { placeholder: '{{ created_month }}', example: '03' },
  day: { placeholder: '{{ created_day }}', example: '17' },
  correspondent: { placeholder: '{{ correspondent }}', example: 'Swisscom' },
  document_type: { placeholder: '{{ document_type }}', example: 'Rechnung' },
  owner: { placeholder: '{{ owner_username }}', example: 'anna' },
  title: { placeholder: '{{ title }}', example: 'Rechnung März' },
  asn: { placeholder: '{{ asn }}', example: '000123' },
}

/** Parts that qualify as a file name (last level) */
export const FILE_PARTS: PartId[] = ['title', 'asn']

export const PRESETS: { id: string; parts: PartId[] }[] = [
  { id: 'year_correspondent', parts: ['year', 'correspondent', 'title'] },
  { id: 'correspondent_year', parts: ['correspondent', 'year', 'title'] },
  { id: 'type_year', parts: ['document_type', 'year', 'title'] },
  { id: 'year_month', parts: ['year', 'month', 'title'] },
]

/** Parts → template; without a file name at the end the title is appended */
export function buildTemplate(parts: PartId[]): string {
  const p = parts.filter((x, i) => parts.indexOf(x) === i)
  if (!p.length) return ''
  if (!FILE_PARTS.includes(p[p.length - 1])) p.push('title')
  return p.map((x) => PARTS[x].placeholder).join('/')
}

/** Template → parts if it only consists of known placeholders (otherwise null = "advanced" template) */
export function parseTemplate(tpl: string): PartId[] | null {
  const t = (tpl ?? '').trim()
  if (!t) return []
  const out: PartId[] = []
  for (const seg of t.split('/')) {
    const s = seg.trim().replace(/\s+/g, ' ')
    const id = (Object.keys(PARTS) as PartId[]).find((k) => PARTS[k].placeholder === s || PARTS[k].placeholder.replace(/ /g, '') === s.replace(/ /g, ''))
    if (!id) return null
    out.push(id)
  }
  return out
}

/** Example path with example values (without server); file extension like upstream (.pdf) */
export function examplePath(parts: PartId[]): string {
  const tpl = buildTemplate(parts)
  if (!tpl) return ''
  const p = parseTemplate(tpl) ?? []
  return p.map((x) => PARTS[x].example).join('/') + '.pdf'
}
