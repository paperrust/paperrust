// Client of the role API (/rs/api/v2/roles, docs/DESIGN2-ENTWIRREN.md §1). If the server does not answer with the API
// (404/405/501/503, HTML/redirect page, 403 for non-administrators), loadRoles() returns the local default of the
// four classic roles (`fallback: true`); the user management then writes only via v1.
import { getCookie } from '../../api/v1'
import { fallbackInfo, sortRoles, type Role, type RoleMode, type RolesInfo, type Topic } from '../admin/roles'

const BASE = '/rs/api/v2/roles'
const UNAVAILABLE = new Set([404, 405, 501, 503])

export class RolesUnavailable extends Error {
  constructor(public status: number) {
    super(`roles api unavailable (${status})`)
  }
}
export class RolesError extends Error {
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
export function setRolesFetch(f: Fetch | null) {
  fetcher = f ?? ((i, init) => fetch(i, init))
}

/** Error text from the response (v2 `{error:{message}}`, DRF `{detail}` or field errors), without internals */
export function errorText(j: any): string | null {
  if (!j || typeof j !== 'object') return null
  if (typeof j.error?.message === 'string') return j.error.message
  if (typeof j.detail === 'string') return j.detail
  if (Array.isArray(j.non_field_errors) && typeof j.non_field_errors[0] === 'string') return j.non_field_errors[0]
  for (const v of Object.values(j)) if (Array.isArray(v) && typeof v[0] === 'string') return v[0]
  return null
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { Accept: 'application/json' }
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  if (method !== 'GET') {
    const csrf = getCookie('csrftoken')
    if (csrf) headers['X-CSRFToken'] = csrf
  }
  const res = await fetcher(BASE + path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) })
  const ct = res.headers.get('Content-Type') ?? ''
  if (UNAVAILABLE.has(res.status) || (res.ok && (res.redirected || ct.includes('text/html')))) throw new RolesUnavailable(res.status)
  if (!res.ok) {
    let msg: string | null = null
    try {
      msg = errorText(await res.json())
    } catch {}
    throw new RolesError(res.status, msg ?? `HTTP ${res.status}`)
  }
  if (res.status === 204) return undefined as T
  const text = await res.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** Response of GET /roles into the UI type (tolerant of missing fields) */
export function normalizeRoles(j: any): RolesInfo {
  const roles: Role[] = (Array.isArray(j?.roles) ? j.roles : []).map((r: any) => ({
    id: r.id,
    key: ['read', 'edit', 'manage', 'admin'].includes(r.key) ? r.key : null,
    name: String(r.name ?? ''),
    description: String(r.description ?? ''),
    builtin: !!r.builtin,
    admin: !!r.admin || r.key === 'admin',
    permissions: Array.isArray(r.permissions) ? r.permissions.map(String) : [],
  }))
  const topics: Topic[] = (Array.isArray(j?.topics) ? j.topics : []).map((t: any) => ({ key: String(t.key), models: Array.isArray(t.models) ? t.models.map(String) : [] }))
  return {
    mode: j?.mode === 'custom' ? 'custom' : 'classic',
    roles: sortRoles(roles),
    topics: topics.length ? topics : fallbackInfo().topics,
    always: Array.isArray(j?.always) ? j.always.map(String) : fallbackInfo().always,
  }
}

export const getRoles = async () => normalizeRoles(await call('GET', ''))

/** Load roles; without a reachable API (or without admin permission) the local default */
export async function loadRoles(): Promise<RolesInfo> {
  try {
    return await getRoles()
  } catch (e) {
    if (e instanceof RolesUnavailable || (e instanceof RolesError && (e.status === 403 || e.status === 401))) return fallbackInfo()
    throw e
  }
}

/** cached role mode (e.g. for the administration navigation: "Roles" only in custom mode) */
let modeCache: Promise<RoleMode | null> | null = null
export function rolesMode(refresh = false): Promise<RoleMode | null> {
  if (!modeCache || refresh)
    modeCache = getRoles()
      .then((i) => i.mode)
      .catch(() => null)
  return modeCache
}

export async function setRoleMode(mode: RoleMode): Promise<void> {
  await call('PUT', '/mode', { mode })
  modeCache = Promise.resolve(mode)
  // e.g. for the administration navigation ("Roles" only in custom mode)
  if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent('paperrust:roles-mode', { detail: mode }))
}
export const createRole = (r: { name: string; description: string; permissions: string[] }) => call<any>('POST', '', r)
export const updateRole = (id: number | string, r: Partial<{ name: string; description: string; permissions: string[] }>) => call<any>('PATCH', `/${encodeURIComponent(String(id))}`, r)
export const deleteRole = (id: number | string) => call<void>('DELETE', `/${encodeURIComponent(String(id))}`)
export const assignRole = (id: number | string, to: { users?: number[]; groups?: number[] }) =>
  call<unknown>('POST', `/${encodeURIComponent(String(id))}/assign`, { users: to.users ?? [], groups: to.groups ?? [] })

export interface Resolved {
  users: Record<string, number | string>
  groups: Record<string, number | string>
}
/** Hint of the API (role_id | "individual"); empty if not available */
export async function resolveRoles(users: number[], groups: number[]): Promise<Resolved> {
  try {
    const j = await call<any>('GET', `/resolve?users=${users.join(',')}&groups=${groups.join(',')}`)
    return { users: j?.users ?? {}, groups: j?.groups ?? {} }
  } catch {
    return { users: {}, groups: {} }
  }
}
