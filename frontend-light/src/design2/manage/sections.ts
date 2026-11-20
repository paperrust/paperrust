// Areas of the administration in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §6): each area appears if the
// view permission for it exists. In the "System" area, users and groups, roles, logs and
// configuration remain reserved for administrators (is_superuser or is_staff) (`adminOnly`); workflows, mail, tasks
// and trash are seen by everyone with the matching permission (the fixed role "Manage" grants workflow/mail permissions;
// trash like upstream app-frame with "Delete document").
// What someone may not do, they do not see.
// Framework-free so that visibility can be tested without Svelte (sections.test.ts).
import { SYMBOL } from '../lib/icons'

export type Area = 'order' | 'system'
/** Role mode from GET /rs/api/v2/roles; 404/error = classic (four fixed roles) */
export type RolesMode = 'classic' | 'custom'

export interface SectionDef {
  /** Route (without base), e.g. attributes/tags */
  path: string
  /** further routes that belong to this area (prefix) */
  also?: string[]
  label: string
  icon: string
  area: Area
  /** Object type for session.can(permAction, …); null = no own permission needed */
  perm: string | null
  /** required action (default "view"; trash like upstream app-frame: "delete" on the document) */
  permAction?: 'view' | 'add' | 'delete'
  /** administrators only (is_superuser or is_staff), in addition to the permission */
  adminOnly?: boolean
  /** superusers only (in addition to the permission) */
  superuserOnly?: boolean
  /** Navigation entry only in role mode "custom" – the route stays reachable in both modes
   *  (the classic/custom switch is located on it) */
  customRolesOnly?: boolean
}

export interface Viewer {
  can: (type: string, action: 'view' | 'add' | 'delete') => boolean
  admin: boolean
  /** Superuser (missing = like admin) */
  superuser?: boolean
  rolesMode: RolesMode
}

/** Route of the area may be opened (independent of the role mode) */
export function sectionAllowed(s: SectionDef, v: Viewer): boolean {
  if (s.adminOnly && !v.admin) return false
  if (s.superuserOnly && !(v.superuser ?? v.admin)) return false
  return s.perm === null || v.can(s.perm, s.permAction ?? 'view')
}

/** Area appears in the navigation */
export function sectionVisible(s: SectionDef, v: Viewer): boolean {
  if (s.customRolesOnly && v.rolesMode !== 'custom') return false
  return sectionAllowed(s, v)
}

export const visibleSections = (all: SectionDef[], v: Viewer) => all.filter((s) => sectionVisible(s, v))
export const allowedSections = (all: SectionDef[], v: Viewer) => all.filter((s) => sectionAllowed(s, v))

/** Area for the current route (exact, then prefix or also-prefixes) */
export function sectionFor(all: SectionDef[], path: string): SectionDef | undefined {
  const hit = (s: SectionDef, p: string) => path === p || path.startsWith(p + '/')
  return all.find((s) => path === s.path) ?? all.find((s) => hit(s, s.path) || (s.also ?? []).some((a) => hit(s, a)))
}

/** Response of GET /rs/api/v2/roles → mode (anything except explicit "custom" = classic) */
export function rolesModeOf(status: number, body: unknown): RolesMode {
  if (status < 200 || status >= 300) return 'classic'
  return body && typeof body === 'object' && (body as { mode?: unknown }).mode === 'custom' ? 'custom' : 'classic'
}

/** Query the mode from the server; role API missing (404) or error → classic */
export async function fetchRolesMode(f: typeof fetch = fetch): Promise<RolesMode> {
  try {
    const r = await f('/rs/api/v2/roles', { headers: { Accept: 'application/json' }, credentials: 'same-origin' })
    return rolesModeOf(r.status, r.ok ? await r.json().catch(() => null) : null)
  } catch {
    return 'classic'
  }
}

/** Icon per area (one meaning = one icon, see SYMBOL in lib/icons.ts) */
export const SECTION_ICON = {
  tags: SYMBOL.tags,
  correspondents: SYMBOL.correspondent,
  documentTypes: SYMBOL.documentType,
  storagePaths: SYMBOL.storagePath,
  customFields: SYMBOL.customField,
  savedViews: SYMBOL.savedView,
  asn: SYMBOL.asnLabels,
  usersGroups: SYMBOL.usersGroups,
  roles: SYMBOL.roles,
  workflows: SYMBOL.workflow,
  mail: SYMBOL.mail,
  tasks: SYMBOL.tasks,
  logs: SYMBOL.logs,
  trash: SYMBOL.trash,
  config: SYMBOL.configuration,
  scan: SYMBOL.scan,
  shareLinks: SYMBOL.shareLinks,
  scanners: SYMBOL.scanners,
  protection: SYMBOL.protection,
} as const
