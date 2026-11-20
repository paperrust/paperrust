// Logic of common/permissions-select/permissions-select.component.ts (framework-free for Vitest).
export const PERMISSION_ACTIONS = { Add: 'add', View: 'view', Change: 'change', Delete: 'delete' } as const
export const PERMISSION_TYPES: Record<string, string> = {
  Document: '%s_document',
  Tag: '%s_tag',
  Correspondent: '%s_correspondent',
  DocumentType: '%s_documenttype',
  StoragePath: '%s_storagepath',
  SavedView: '%s_savedview',
  PaperlessTask: '%s_paperlesstask',
  AppConfig: '%s_applicationconfiguration',
  UISettings: '%s_uisettings',
  History: '%s_logentry',
  Note: '%s_note',
  MailAccount: '%s_mailaccount',
  MailRule: '%s_mailrule',
  User: '%s_user',
  Group: '%s_group',
  ShareLink: '%s_sharelink',
  ShareLinkBundle: '%s_sharelinkbundle',
  CustomField: '%s_customfield',
  Workflow: '%s_workflow',
  ProcessedMail: '%s_processedmail',
  GlobalStatistics: '%s_global_statistics',
  SystemMonitoring: '%s_system_monitoring',
}
export type ActionKey = keyof typeof PERMISSION_ACTIONS
const ACTION_ORDER = ['add', 'change', 'delete', 'view']
/** KeyValuePipe + sortActions: Add, Change, Delete, View */
export const SORTED_ACTIONS = (Object.keys(PERMISSION_ACTIONS) as ActionKey[]).sort((a, b) => ACTION_ORDER.indexOf(PERMISSION_ACTIONS[a]) - ACTION_ORDER.indexOf(PERMISSION_ACTIONS[b]))

export const permissionCode = (action: ActionKey, type: string) => PERMISSION_TYPES[type].replace('%s', PERMISSION_ACTIONS[action])

export function allowedTypes(auditlogEnabled: boolean): string[] {
  const t = Object.keys(PERMISSION_TYPES)
  if (!auditlogEnabled) t.splice(t.indexOf('History'), 1)
  return t
}

/** Global statistics and system status only know "view" */
export function isActionSupported(type: string, action: ActionKey): boolean {
  if (type === 'GlobalStatistics' || type === 'SystemMonitoring') return action === 'View'
  return true
}

/** inheritedPermissions setter: remove <app_label>. */
export const stripAppLabel = (perms: string[] | null | undefined) => (perms?.length ? perms.map((p) => p.replace(/^\w+\./g, '')) : [])

export function isInherited(inherited: string[], type: string, action: ActionKey | null = null): boolean {
  if (inherited.length == 0) return false
  if (action) return inherited.includes(permissionCode(action, type))
  return (Object.keys(PERMISSION_ACTIONS) as ActionKey[]).filter((a) => isActionSupported(type, a)).every((a) => inherited.includes(permissionCode(a, type)))
}

/** Checkmark per type/action from own + inherited permissions (applyCheckedState) */
export function checkedState(types: string[], own: string[], inherited: string[]): Record<string, Record<ActionKey, boolean>> {
  const all = new Set([...inherited, ...own])
  const out: Record<string, Record<ActionKey, boolean>> = {}
  for (const type of types) {
    const row = {} as Record<ActionKey, boolean>
    for (const a of Object.keys(PERMISSION_ACTIONS) as ActionKey[]) row[a] = isActionSupported(type, a) && all.has(permissionCode(a, type))
    out[type] = row
  }
  return out
}

export const typeHasAllActions = (row: Record<ActionKey, boolean>, type: string) =>
  (Object.keys(PERMISSION_ACTIONS) as ActionKey[]).filter((a) => isActionSupported(type, a)).every((a) => !!row?.[a])

/**
 * getSelectedPermissions over the enabled (non-inherited) checkboxes, without inherited permissions – corresponds to
 * form.valueChanges → onChange (disabled controls are missing in form.value).
 */
export function selectedPermissions(state: Record<string, Record<ActionKey, boolean>>, inherited: string[]): string[] {
  const out: string[] = []
  for (const [type, row] of Object.entries(state)) {
    for (const a of Object.keys(PERMISSION_ACTIONS) as ActionKey[]) {
      if (!row[a] || !isActionSupported(type, a) || isInherited(inherited, type, a)) continue
      out.push(permissionCode(a, type))
    }
  }
  return out.filter((p) => !inherited.includes(p))
}
