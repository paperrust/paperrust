// "Visible to" (docs/DESIGN2-ENTWIRREN.md §3): quick choice on top of the existing object permissions (owner +
// set_permissions). Pure logic without Svelte, tested in visibility.test.ts.
//
// Upstream semantics (paperless-ngx v3.4.2, upstream/src/documents):
//  - permissions.py `permitted_object_ids`: A document is visible to superusers, the owner, users/groups
//    with object permission `view_document` – and to EVERY logged-in, active user if `owner` is empty
//    (`owner__isnull=True`). The list then filters; the global permission `view_document` is always needed.
//  - permissions.py `PaperlessObjectPermissions.has_object_permission`: without owner → anyone with the global
//    permission may also change; with owner → owner or object permission `change_document`.
//  - permissions.py `set_permissions_for_objects`: "change" grants "view" as well; every given action replaces
//    the direct permissions (merge=false). The read representation (`permissions` with full_perms=true) therefore shows
//    groups with change permission also under "view".
//  - serialisers.py `OwnedObjectSerializer.update`: only superusers, the owner or, for documents without owner,
//    anyone may change owner/permissions (otherwise 403 "Insufficient permissions.").
//  - bulk_edit.py `set_permissions`: merge=false ALWAYS sets `owner` (parameter missing → owner = None!),
//    views.py `BulkEditView._has_document_permissions`: only superusers or the owner of all documents
//    (or documents without owner).
//
// Mapping (chosen):
//  - Everyone           = owner: null, no object permissions. Upstream logic "no owner = everyone sees" – the
//                         only means that really covers every user (view for all groups would not cover a
//                         user without a group). Consequence: whoever has the global change permission may also change
//                         the document; the UI says so in the hint.
//  - My team            = owner stays (without owner: the current user), view + edit for exactly
//                         the groups of the owner, no users.
//  - Only me            = owner stays (without owner: the current user), no object permissions. If the
//                         document belongs to someone else (admin view), the choice is called "Owner only (name)".
//  - Specific people    = everything else; opens the detailed selection owner / view / edit.
// Detection (classify) is strict: only exact hits show a quick choice, everything else is "Specific people"
// with a summary. Detection never changes anything; permissions are written only when the user chooses.
// Bulk edit: one call for all documents → owner for "My team"/"Only me" = current user,
// team = his groups; owner is always sent explicitly.

export type VisibilityMode = 'all' | 'team' | 'private' | 'custom'

export interface Grantees {
  users: number[]
  groups: number[]
}
export interface Perms {
  view: Grantees
  change: Grantees
}
export interface VisibilityValue {
  owner: number | null
  set_permissions: Perms
}

export const emptyPerms = (): Perms => ({ view: { users: [], groups: [] }, change: { users: [], groups: [] } })

/** Normalize permissions (missing parts = empty, duplicates removed, sorted) */
export function normPerms(p: Partial<Perms> | null | undefined): Perms {
  const g = (x: Partial<Grantees> | undefined): Grantees => ({ users: uniq(x?.users), groups: uniq(x?.groups) })
  return { view: g(p?.view), change: g(p?.change) }
}
function uniq(a: number[] | undefined): number[] {
  return [...new Set(a ?? [])].sort((x, y) => x - y)
}
const sameSet = (a: number[], b: number[]) => {
  const x = uniq(a)
  const y = uniq(b)
  return x.length === y.length && x.every((v, i) => v === y[i])
}
const isEmpty = (p: Perms) => !p.view.users.length && !p.view.groups.length && !p.change.users.length && !p.change.groups.length

/**
 * Map existing permissions to a quick choice.
 * @param ownerGroups groups of the owner; `null` = unknown (then never "team")
 */
export function classify(owner: number | null, permissions: Partial<Perms> | null | undefined, ownerGroups: number[] | null): VisibilityMode {
  const p = normPerms(permissions)
  if (owner === null) return isEmpty(p) ? 'all' : 'custom'
  if (isEmpty(p)) return 'private'
  if (
    ownerGroups &&
    ownerGroups.length > 0 &&
    !p.view.users.length &&
    !p.change.users.length &&
    sameSet(p.change.groups, ownerGroups) &&
    // View ⊆ team (change grants view; older data without the view row also counts)
    sameSet([...p.view.groups, ...p.change.groups], ownerGroups)
  )
    return 'team'
  return 'custom'
}

export interface ApplyContext {
  /** previous owner (single document) or null */
  owner: number | null
  /** current user */
  me: number
  /** Groups of the (future) owner; null = unknown */
  ownerGroups: number[] | null
}

/** Owner that applies for "My team"/"Only me" */
export const effectiveOwner = (ctx: Pick<ApplyContext, 'owner' | 'me'>) => ctx.owner ?? ctx.me

/** Is the quick choice possible? ("My team" needs known, non-empty groups) */
export function available(mode: VisibilityMode, ownerGroups: number[] | null): boolean {
  return mode !== 'team' || (!!ownerGroups && ownerGroups.length > 0)
}

/** Quick choice → value for PATCH (owner + set_permissions) or bulk_edit set_permissions. `custom` → null. */
export function apply(mode: VisibilityMode, ctx: ApplyContext): VisibilityValue | null {
  switch (mode) {
    case 'all':
      return { owner: null, set_permissions: emptyPerms() }
    case 'private':
      return { owner: effectiveOwner(ctx), set_permissions: emptyPerms() }
    case 'team': {
      if (!available('team', ctx.ownerGroups)) return null
      const g = uniq(ctx.ownerGroups ?? [])
      return { owner: effectiveOwner(ctx), set_permissions: { view: { users: [], groups: [...g] }, change: { users: [], groups: [...g] } } }
    }
    default:
      return null
  }
}

/** May the user change owner/permissions? (serialisers.py OwnedObjectSerializer.update) */
export function canChangeVisibility(owner: number | null, user: { id: number; is_superuser?: boolean } | null | undefined): boolean {
  if (!user) return false
  return !!user.is_superuser || owner === null || owner === user.id
}

/** Bulk edit allowed? (views.py BulkEditView._has_document_permissions for set_permissions) */
export function canBulkChangeVisibility(owners: (number | null)[], user: { id: number; is_superuser?: boolean } | null | undefined): boolean {
  if (!user) return false
  return !!user.is_superuser || owners.every((o) => o === null || o === user.id)
}

/** Parameters for POST /api/documents/bulk_edit/ method=set_permissions (owner always explicit) */
export function bulkParameters(v: VisibilityValue, merge = false): { set_permissions: Perms; owner: number | null; merge: boolean } {
  return { set_permissions: normPerms(v.set_permissions), owner: v.owner, merge }
}

export interface SummaryLabels {
  owner: string
  noOwner: string
  view: string
  change: string
  nobody: string
}

/**
 * Short summary for "Specific people", e.g. "Owner: alice · View: bob, Team A · Edit: Team A".
 * Groups with change permission are not listed twice under view.
 */
export function summarize(owner: number | null, permissions: Partial<Perms> | null | undefined, userName: (id: number) => string, groupName: (id: number) => string, l: SummaryLabels): string {
  const p = normPerms(permissions)
  const names = (g: Grantees, minus?: Grantees) => [
    ...g.users.filter((u) => !minus?.users.includes(u)).map(userName),
    ...g.groups.filter((x) => !minus?.groups.includes(x)).map(groupName),
  ]
  const view = names(p.view, p.change)
  const change = names(p.change)
  const parts = [owner === null ? l.noOwner : `${l.owner}: ${userName(owner)}`]
  if (view.length) parts.push(`${l.view}: ${view.join(', ')}`)
  if (change.length) parts.push(`${l.change}: ${change.join(', ')}`)
  if (!view.length && !change.length && owner !== null) parts.push(`${l.view}: ${l.nobody}`)
  return parts.join(' · ')
}
