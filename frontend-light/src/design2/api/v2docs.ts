// Autosave via API v2 with `If-Match` (docs/DESIGN2.md §4.3): determine document UUID/rev for the v1 document,
// translate v1-shaped changes into a v2 `DocumentPatch` (references as UUID). Where that is not possible
// (no v2, document not found, field without mapping), the caller decides for v1.
import { getCookie, type Document } from '../../api/v1'

const V2 = '/rs/api/v2'

export interface V2Ref {
  uuid: string
  rev: number
}

export class Conflict extends Error {
  constructor(public currentRev: number | null) {
    super('precondition_failed')
  }
}

type Kind = 'tags' | 'correspondents' | 'document_types' | 'storage_paths' | 'custom_fields'
export type IdMaps = Record<Kind, Map<number, string>>

async function req(method: string, path: string, body?: unknown, headers: Record<string, string> = {}) {
  const h: Record<string, string> = { Accept: 'application/json', ...headers }
  if (body !== undefined) h['Content-Type'] = 'application/json'
  if (method !== 'GET') {
    const c = getCookie('csrftoken')
    if (c) h['X-CSRFToken'] = c
  }
  return fetch(V2 + path, { method, headers: h, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) })
}

let maps: Promise<IdMaps | null> | null = null
/** legacy_id → UUID per object type (once per session; null = v2 not usable) */
export function idMaps(): Promise<IdMaps | null> {
  maps ??= (async () => {
    const kinds: Kind[] = ['tags', 'correspondents', 'document_types', 'storage_paths', 'custom_fields']
    const out = {} as IdMaps
    for (const k of kinds) {
      const m = new Map<number, string>()
      let cursor: string | null = null
      do {
        const r = await req('GET', `/${k}?limit=1000${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ''}`)
        if (!r.ok || !(r.headers.get('Content-Type') ?? '').includes('json')) return null
        const j = await r.json()
        for (const o of j.items ?? []) m.set(Number(o.legacy_id), String(o.uuid))
        cursor = j.next_cursor ?? null
      } while (cursor)
      out[k] = m
    }
    return out
  })().catch(() => null)
  return maps
}
/** reload after creating new objects (administration) */
export const resetIdMaps = () => (maps = null)

/** UUID + rev of a v1 document via `POST /documents/query` (narrow by date + title, compare legacy_id) */
export async function resolveDoc(d: Pick<Document, 'id' | 'title' | 'created'>): Promise<V2Ref | null> {
  try {
    const day = d.created.slice(0, 10)
    const filter = { all: [{ created: { from: day, to: day } }, { text: { query: d.title, field: 'title' } }] }
    const r = await req('POST', '/documents/query', { filter, limit: 100 })
    if (!r.ok || !(r.headers.get('Content-Type') ?? '').includes('json')) return null
    const j = await r.json()
    const hit = (j.items ?? []).find((x: any) => Number(x.legacy_id) === d.id)
    return hit ? { uuid: String(hit.uuid), rev: Number(hit.rev) } : null
  } catch {
    return null
  }
}

/** v1-shaped change → v2 DocumentPatch; null if a value cannot be mapped */
export function toV2Patch(p: Record<string, unknown>, m: IdMaps): Record<string, unknown> | null {
  const out: Record<string, unknown> = {}
  const ref = (k: Kind, v: unknown): string | null | undefined => (v === null ? null : m[k].get(Number(v)))
  for (const [k, v] of Object.entries(p)) {
    switch (k) {
      case 'title':
      case 'created':
        out[k] = v
        break
      case 'archive_serial_number':
        out.asn = v
        break
      case 'correspondent':
      case 'document_type':
      case 'storage_path': {
        const u = ref(k === 'correspondent' ? 'correspondents' : k === 'document_type' ? 'document_types' : 'storage_paths', v)
        if (u === undefined) return null
        out[k] = u
        break
      }
      case 'tags': {
        const us = (v as number[]).map((id) => m.tags.get(id))
        if (us.some((x) => !x)) return null
        out.tags = us
        break
      }
      case 'custom_fields': {
        const cfs: { field: string; value: unknown }[] = []
        for (const c of v as { field: number; value: unknown }[]) {
          const f = m.custom_fields.get(c.field)
          // Document links are UUID lists in v2 → v1 takes over
          if (!f || Array.isArray(c.value)) return null
          cfs.push({ field: f, value: c.value })
        }
        out.custom_fields = cfs
        break
      }
      default:
        return null
    }
  }
  return out
}

/** PATCH /documents/{uuid} with If-Match; returns the new rev (ETag) */
export async function patchV2(ref: V2Ref, patch: Record<string, unknown>): Promise<number> {
  const r = await req('PATCH', `/documents/${ref.uuid}`, patch, { 'If-Match': `"${ref.rev}"` })
  if (r.status === 412) {
    let cur: number | null = Number((r.headers.get('ETag') ?? '').replace(/"/g, '')) || null
    try {
      cur = (await r.json())?.error?.current_rev ?? cur
    } catch {}
    throw new Conflict(cur)
  }
  if (!r.ok) {
    let body: unknown = null
    try {
      body = await r.json()
    } catch {}
    const msg = (body as any)?.error?.message ?? `HTTP ${r.status}`
    const fields = (body as any)?.error?.fields
    throw Object.assign(new Error(msg), { status: r.status, body: fields ?? body })
  }
  const etag = Number((r.headers.get('ETag') ?? '').replace(/"/g, ''))
  if (Number.isFinite(etag) && etag > 0) return etag
  const j = await r.json().catch(() => null)
  return Number(j?.rev ?? ref.rev)
}
