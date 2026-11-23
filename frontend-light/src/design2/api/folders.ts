// Folder view (PaperRust only): client of /rs/api/v2/folders (crates/pl-api-v2/src/folders.rs, contract
// docs/contract/pl-api-v2.md "Ordner") and pure tree helpers. A document lies in at most one folder; no folder =
// "Unsorted". The document list itself stays the v1 list with the filters folder__id / folder__isnull.
import { getCookie } from '../../api/v1'
import { errorText } from './roles'

const BASE = '/rs/api/v2/folders'

export interface Folder {
  id: number
  uuid: string
  rev: number
  name: string
  parent: number | null
  owner: number | null
  color: string | null
  sort: number
  user_can_change: boolean
  user_can_delete: boolean
  document_count?: number
}

export interface FolderList {
  items: Folder[]
  unsorted: number | null
  can_organize: boolean
}

/** Folder of the route: id or the virtual "Unsorted" */
export type FolderKey = number | 'unsorted'

export class FolderError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
  }
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>
let fetcher: Fetch = (i, init) => fetch(i, init)
/** tests only */
export function setFoldersFetch(f: Fetch | null) {
  fetcher = f ?? ((i, init) => fetch(i, init))
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' }
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  if (method !== 'GET') {
    const csrf = getCookie('csrftoken')
    if (csrf) headers['X-CSRFToken'] = csrf
  }
  const res = await fetcher(BASE + path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) })
  if (!res.ok) {
    let msg: string | null = null
    try {
      msg = errorText(await res.json())
    } catch {}
    throw new FolderError(res.status, msg ?? `HTTP ${res.status}`)
  }
  if (res.status === 204) return undefined as T
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

export const loadFolders = () => call<FolderList>('GET', '')
export const createFolder = (name: string, parent: number | null) => call<Folder>('POST', '', { name, parent })
export const renameFolder = (id: number, name: string) => call<Folder>('PATCH', `/${id}`, { name })
export const moveFolder = (id: number, parent: number | null) => call<Folder>('PATCH', `/${id}`, { parent })
export const deleteFolder = (id: number, contents: 'parent' | 'unsorted') =>
  call<{ moved_documents: number; moved_folders: number }>('DELETE', `/${id}?contents=${contents}`)
export const moveDocuments = (documents: number[], folder: number | null) =>
  call<{ moved: number; folder: number | null }>('POST', '/move-documents', { documents, folder })

/** v1 list parameters of a folder */
export function folderParams(key: FolderKey): Record<string, string> {
  return key === 'unsorted' ? { folder__isnull: '1' } : { folder__id: String(key) }
}

export interface TreeRow {
  folder: Folder
  depth: number
  hasChildren: boolean
}

/**
 * Depth-first rows of the tree (siblings in server order: sort, name). Folders whose parent is not visible are
 * roots. `collapsed` hides the subtrees of these ids.
 */
export function treeRows(items: Folder[], collapsed: ReadonlySet<number> = new Set()): TreeRow[] {
  const ids = new Set(items.map((f) => f.id))
  const children = new Map<number | null, Folder[]>()
  for (const f of items) {
    const p = f.parent !== null && ids.has(f.parent) ? f.parent : null
    const list = children.get(p) ?? []
    list.push(f)
    children.set(p, list)
  }
  const out: TreeRow[] = []
  const seen = new Set<number>()
  const visit = (parent: number | null, depth: number) => {
    for (const f of children.get(parent) ?? []) {
      if (seen.has(f.id)) continue
      seen.add(f.id)
      const kids = children.get(f.id) ?? []
      out.push({ folder: f, depth, hasChildren: kids.length > 0 })
      if (!collapsed.has(f.id)) visit(f.id, depth + 1)
    }
  }
  visit(null, 0)
  return out
}

/** Path from the (visible) root to `id`, including the folder itself */
export function folderPath(items: Folder[], id: number): Folder[] {
  const by = new Map(items.map((f) => [f.id, f]))
  const out: Folder[] = []
  const seen = new Set<number>()
  let cur = by.get(id)
  while (cur && !seen.has(cur.id)) {
    seen.add(cur.id)
    out.unshift(cur)
    cur = cur.parent !== null ? by.get(cur.parent) : undefined
  }
  return out
}

/** Picker options: name with path ("Invoices › 2026"), sorted like the tree */
export function folderOptions(items: Folder[], only: (f: Folder) => boolean = () => true): { id: number; name: string; color?: string }[] {
  return treeRows(items)
    .filter((r) => only(r.folder))
    .map((r) => ({
      id: r.folder.id,
      name: folderPath(items, r.folder.id)
        .map((f) => f.name)
        .join(' › '),
      color: r.folder.color ?? undefined,
    }))
}

/** Route `folders[/unsorted|/<id>][/<doc>]` → folder key + active document */
export function parseFolderRoute(path: string): { key: FolderKey; activeId: number | null } | null {
  const m = /^folders(?:\/(unsorted|\d+)(?:\/(\d+))?)?$/.exec(path)
  if (!m) return null
  const key: FolderKey = !m[1] || m[1] === 'unsorted' ? 'unsorted' : +m[1]
  return { key, activeId: m[2] ? +m[2] : null }
}

export function folderRoute(key: FolderKey, docId: number | null = null): string {
  return `folders/${key}` + (docId ? `/${docId}` : '')
}

/** UI setting (per user, ui_settings.paperrust.folders): folder view on/off, default off */
export const FOLDERS_KEY = 'paperrust:folders'
export function foldersEnabled(settings: Record<string, any> | null | undefined): boolean {
  return settings?.paperrust?.folders === true
}
