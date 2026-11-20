// Role logic of the user management in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §1–2), framework-free for
// Vitest. Roles are a layer over the Django permissions: one role = named set of code names; the truth
// remains `user_permissions` / group `permissions` (v1, code names without app) and `is_superuser`.
// Internally code names WITHOUT app prefix are used (as v1 stores them); the role API delivers `app.codename`.

export type RoleMode = 'classic' | 'custom'
export type RoleKey = 'read' | 'edit' | 'manage' | 'admin'
export type TopicKey = 'documents' | 'organize' | 'views' | 'automation' | 'system'
export type Action = 'view' | 'edit' | 'delete'
export const ACTIONS: Action[] = ['view', 'edit', 'delete']
export const TOPIC_ORDER: TopicKey[] = ['documents', 'organize', 'views', 'automation', 'system']

export interface Role {
  id: number | string
  key: RoleKey | null
  name: string
  description: string
  builtin: boolean
  admin: boolean
  /** `app.codename` (API) or code names */
  permissions: string[]
}
export interface Topic {
  key: TopicKey | string
  /** `app.model` */
  models: string[]
}
export interface RolesInfo {
  mode: RoleMode
  roles: Role[]
  topics: Topic[]
  /** always included permissions (`app.codename`, e.g. ui_settings) */
  always: string[]
  /** true = role API not reachable, local default of the four classic roles */
  fallback?: boolean
}

/** `documents.view_document` → `view_document` */
export const bare = (p: string) => p.replace(/^[A-Za-z0-9_]+\./, '')
const bareSet = (ps: Iterable<string>) => new Set([...ps].map(bare))

/** Code names of a switch for a model `app.model`: view = view, edit = add + change, delete = delete */
export function actionCodenames(model: string, action: Action): string[] {
  const m = bare(model)
  if (action === 'view') return [`view_${m}`]
  if (action === 'edit') return [`add_${m}`, `change_${m}`]
  return [`delete_${m}`]
}
export const topicCodenames = (topic: Topic, action: Action) => topic.models.flatMap((m) => actionCodenames(m, action))
/** all code names covered by the topic switches */
export const coveredCodenames = (topics: Topic[]) => new Set(topics.flatMap((t) => ACTIONS.flatMap((a) => topicCodenames(t, a))))

export type Switches = Record<string, Record<Action, boolean>>
export type SwitchState = Record<string, Record<Action, boolean | 'mixed'>>

/** Switches → code names (sorted, without duplicates), `always` is included */
export function switchesToPerms(sw: Switches, topics: Topic[], always: string[] = []): string[] {
  const out = new Set(always.map(bare))
  for (const t of topics) for (const a of ACTIONS) if (sw[t.key]?.[a]) for (const c of topicCodenames(t, a)) out.add(c)
  return [...out].sort()
}

/** Code names → switches: on = all code names present, off = none, 'mixed' = partial */
export function permsToSwitches(perms: Iterable<string>, topics: Topic[]): SwitchState {
  const have = bareSet(perms)
  const out: SwitchState = {}
  for (const t of topics) {
    const row = {} as Record<Action, boolean | 'mixed'>
    for (const a of ACTIONS) {
      const cs = topicCodenames(t, a)
      const n = cs.filter((c) => have.has(c)).length
      row[a] = n === 0 ? false : n === cs.length ? true : 'mixed'
    }
    out[t.key] = row
  }
  return out
}

/** State of the three switches of a single model (`app.model`) */
export const modelState = (perms: Iterable<string>, model: string) => permsToSwitches(perms, [{ key: model, models: [model] }])[model]
/** Set switches of a single model; everything else stays unchanged */
export const applyModelSwitch = (perms: Iterable<string>, model: string, action: Action, on: boolean) => applySwitch(perms, [{ key: model, models: [model] }], model, action, on)

/** set one switch; all other permissions (also those outside the topics) stay unchanged */
export function applySwitch(perms: Iterable<string>, topics: Topic[], topicKey: string, action: Action, on: boolean): string[] {
  const out = bareSet(perms)
  const t = topics.find((x) => x.key === topicKey)
  if (t) for (const c of topicCodenames(t, action)) on ? out.add(c) : out.delete(c)
  return [...out].sort()
}

/** Can the set be represented losslessly with switches (no subsets, nothing outside the topics)? */
export function representable(perms: Iterable<string>, topics: Topic[], always: string[] = []): boolean {
  const have = bareSet(perms)
  const covered = coveredCodenames(topics)
  const alw = bareSet(always)
  for (const p of have) if (!covered.has(p) && !isAlways(p, alw)) return false
  const st = permsToSwitches(have, topics)
  return Object.values(st).every((row) => ACTIONS.every((a) => row[a] !== 'mixed'))
}

/** always included: `always` of the API and every `*_uisettings` (the UI needs them; never counts toward the role) */
const isAlways = (p: string, alw: Set<string>) => alw.has(p) || p.endsWith('_uisettings')

/** Sets equal (without app prefix, `always` does not count) */
export function samePerms(a: Iterable<string>, b: Iterable<string>, always: string[] = []): boolean {
  const alw = bareSet(always)
  const x = [...bareSet(a)].filter((p) => !isAlways(p, alw))
  const y = new Set([...bareSet(b)].filter((p) => !isAlways(p, alw)))
  return x.length === y.size && x.every((p) => y.has(p))
}

/** own permissions without the always included ones */
export function withoutAlways(perms: Iterable<string>, always: string[] = []): string[] {
  const alw = bareSet(always)
  return [...bareSet(perms)].filter((p) => !isAlways(p, alw)).sort()
}

/** Permissions of a role as code names (+ `always`); administrator = superuser, no individual permissions */
export const rolePerms = (role: Role, always: string[] = []) => (role.admin ? [] : [...new Set([...role.permissions.map(bare), ...always.map(bare)])].sort())

/** Detection context; in classic mode only the fixed roles count */
export type RoleCtx = Pick<RolesInfo, 'roles' | 'always'> & { mode?: RoleMode }
export const activeRoles = (info: RoleCtx) => (info.mode === 'classic' ? info.roles.filter((r) => r.builtin) : info.roles)

export type Classification = { kind: 'role'; role: Role } | { kind: 'individual' } | { kind: 'none' }

/**
 * Role of a permission set: the role with exactly the same set (without `always`); several hits → `hint` (hint of the
 * role API), otherwise the first in list order. Empty set → 'none'. Administrator only via `superuser`.
 */
export function classify(
  perms: Iterable<string>,
  info: RoleCtx,
  opts: { superuser?: boolean; hint?: number | string | null; resolved?: number | string | null } = {},
): Classification {
  if (opts.superuser) {
    const admin = activeRoles(info).find((r) => r.admin)
    return admin ? { kind: 'role', role: admin } : { kind: 'individual' }
  }
  const own = withoutAlways(perms, info.always)
  if (!own.length) return { kind: 'none' }
  // Result of GET /roles/resolve takes precedence (the server knows the permissions present in the DB)
  if (opts.resolved === 'individual') return { kind: 'individual' }
  if (opts.resolved != null) {
    const r = activeRoles(info).find((x) => String(x.id) === String(opts.resolved) && !x.admin)
    if (r) return { kind: 'role', role: r }
  }
  const hits = activeRoles(info).filter((r) => !r.admin && samePerms(own, r.permissions, info.always))
  if (!hits.length) return { kind: 'individual' }
  return { kind: 'role', role: hits.find((r) => opts.hint != null && String(r.id) === String(opts.hint)) ?? hits[0] }
}

export interface UserLike {
  id: number
  is_superuser?: boolean
  is_active?: boolean
  groups?: number[]
  user_permissions?: string[]
}
export interface GroupLike {
  id: number
  name: string
  permissions?: string[]
}
export type Access = Classification & { inherited?: GroupLike | null }

/**
 * Display role of a user: superuser → administrator; own permissions → their role or Individual; without own
 * permissions → role of the groups ("from group X"), otherwise 'none'.
 */
export function userAccess(
  user: UserLike,
  groups: GroupLike[],
  info: RoleCtx,
  resolved?: number | string | null,
  groupResolved: Record<string, number | string> = {},
): Access {
  if (user.is_superuser) return classify([], info, { superuser: true })
  const own = withoutAlways(user.user_permissions ?? [], info.always)
  if (own.length) return { ...classify(own, info, { resolved }), inherited: null }
  const mine = (user.groups ?? []).map((id) => groups.find((g) => g.id === id)).filter((g): g is GroupLike => !!g)
  const withRights = mine.filter((g) => withoutAlways(g.permissions ?? [], info.always).length)
  // exactly one group with permissions: its role (detected by the server); otherwise detect the union locally
  const union = mine.flatMap((g) => g.permissions ?? [])
  const c = withRights.length === 1 ? classify(union, info, { resolved: groupResolved[String(withRights[0].id)] ?? null }) : classify(union, info)
  if (c.kind === 'none') return { kind: 'none', inherited: null }
  const withPerms = mine.filter((g) => withoutAlways(g.permissions ?? [], info.always).length)
  const from =
    c.kind === 'role'
      ? (withPerms.find((g) => {
          const gc = classify(g.permissions ?? [], info, { resolved: groupResolved[String(g.id)] ?? null })
          return gc.kind === 'role' && gc.role.id === c.role.id
        }) ?? withPerms[0])
      : withPerms[0]
  return { ...c, inherited: from ?? null }
}

/** effective permissions = own ∪ groups (code names) */
export function effectivePerms(user: UserLike, groups: GroupLike[]): string[] {
  const out = bareSet(user.user_permissions ?? [])
  for (const id of user.groups ?? []) for (const p of groups.find((g) => g.id === id)?.permissions ?? []) out.add(bare(p))
  return [...out].sort()
}

export type Level = 'none' | 'view' | 'edit' | 'full' | 'partial'
/** Level of a topic from its three switches */
export function levelOf(row: Record<Action, boolean | 'mixed'>): Level {
  const { view: v, edit: e, delete: d } = row
  if (v === 'mixed' || e === 'mixed' || d === 'mixed') return 'partial'
  if (!v && !e && !d) return 'none'
  if (v && !e && !d) return 'view'
  if (v && e && !d) return 'edit'
  if (v && e && d) return 'full'
  return 'partial'
}

/** `all` = administrator (everything allowed, `items` empty) */
export type Summary = { all: boolean; items: { topic: string; level: Level }[] }
/** Summary of the effective permissions per topic (System only if visible) */
export function summarize(perms: Iterable<string>, topics: Topic[], superuser = false): Summary {
  if (superuser) return { all: true, items: [] }
  const st = permsToSwitches(perms, topics)
  return { all: false, items: topics.map((t) => ({ topic: String(t.key), level: levelOf(st[t.key]) })) }
}

/** Code name → `app.codename` for the role API (via topic models, `always` and known role permissions) */
export function qualify(perms: Iterable<string>, info: Pick<RolesInfo, 'topics' | 'always' | 'roles'>): string[] {
  const map = new Map<string, string>()
  for (const t of info.topics)
    for (const m of t.models) {
      const app = m.includes('.') ? m.slice(0, m.indexOf('.')) : ''
      for (const a of ACTIONS) for (const c of actionCodenames(m, a)) if (app) map.set(c, `${app}.${c}`)
    }
  for (const p of [...info.always, ...info.roles.flatMap((r) => r.permissions)]) if (p.includes('.')) map.set(bare(p), p)
  return [...new Set([...perms].map((p) => (p.includes('.') ? p : (map.get(p) ?? p))))].sort()
}

/** Topics the editor sees: "System" for administrators only */
export const visibleTopics = (topics: Topic[], editorIsAdmin: boolean) => topics.filter((t) => editorIsAdmin || t.key !== 'system')

/** Switches of a topic that the editor does not see stay unchanged on save */
export function keepHidden(next: string[], prev: string[], hidden: Topic[]): string[] {
  const hiddenCodes = coveredCodenames(hidden)
  const out = new Set(next.filter((p) => !hiddenCodes.has(bare(p))).map(bare))
  for (const p of prev) if (hiddenCodes.has(bare(p))) out.add(bare(p))
  return [...out].sort()
}

// ---- local default (role API not reachable): four classic roles per DESIGN2-ENTWIRREN §1 ----------------
export const FALLBACK_TOPICS: Topic[] = [
  { key: 'documents', models: ['documents.document', 'documents.note', 'documents.sharelink', 'documents.sharelinkbundle', 'auditlog.logentry'] },
  { key: 'organize', models: ['documents.tag', 'documents.correspondent', 'documents.documenttype', 'documents.storagepath', 'documents.customfield'] },
  { key: 'views', models: ['documents.savedview'] },
  { key: 'automation', models: ['documents.workflow', 'paperless_mail.mailaccount', 'paperless_mail.mailrule', 'paperless_mail.processedmail', 'documents.paperlesstask'] },
  { key: 'system', models: ['auth.user', 'auth.group', 'paperless.applicationconfiguration'] },
]
export const FALLBACK_ALWAYS = ['documents.add_uisettings', 'documents.change_uisettings', 'documents.view_uisettings']

const D = (acts: string[], models: string[]) => acts.flatMap((a) => models.map((m) => `${a}_${m}`))
/** Permissions of the fixed roles like the role API (docs/contract/pl-api-v2.md "Rollen", `roles::{READ,EDIT_EXTRA,MANAGE_EXTRA}`) */
const READ = D(['view'], ['correspondent', 'customfield', 'document', 'documenttype', 'note', 'savedview', 'sharelink', 'sharelinkbundle', 'storagepath', 'tag'])
const EDIT = [...READ, 'add_document', 'change_document', 'add_note', 'change_note', 'add_savedview', 'change_savedview', 'add_sharelink', 'add_sharelinkbundle', 'change_sharelinkbundle']
const MANAGE = [
  ...EDIT,
  ...D(['delete'], ['document', 'note', 'sharelink', 'sharelinkbundle', 'savedview']),
  ...D(['add', 'change', 'delete'], ['tag', 'correspondent', 'documenttype', 'storagepath', 'customfield']),
  ...D(['view', 'add', 'change', 'delete'], ['workflow', 'mailaccount', 'mailrule', 'processedmail']),
  'view_paperlesstask',
  'view_logentry',
]
function fallbackRole(id: number, key: RoleKey, codes: string[]): Role {
  const perms = qualify(codes, { topics: FALLBACK_TOPICS, always: FALLBACK_ALWAYS, roles: [] })
  return { id, key, name: '', description: '', builtin: true, admin: key === 'admin', permissions: perms }
}
export function fallbackInfo(): RolesInfo {
  return {
    mode: 'classic',
    fallback: true,
    topics: FALLBACK_TOPICS,
    always: FALLBACK_ALWAYS,
    roles: [fallbackRole(-1, 'read', READ), fallbackRole(-2, 'edit', EDIT), fallbackRole(-3, 'manage', MANAGE), fallbackRole(-4, 'admin', [])],
  }
}

/** Fixed roles first in the order Read only, Edit, Manage, Administrator, then custom ones by name */
export function sortRoles(roles: Role[]): Role[] {
  const rank = (r: Role) => (r.key ? ['read', 'edit', 'manage', 'admin'].indexOf(r.key) : 10)
  return [...roles].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name))
}

/** Roles available for selection in the mode: classic only the fixed ones, custom all */
export const pickableRoles = (info: RolesInfo, forGroup = false, editorIsAdmin = true) =>
  sortRoles(info.roles.filter((r) => (info.mode === 'custom' || r.builtin) && (!r.admin || (!forGroup && editorIsAdmin))))
