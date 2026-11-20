// Shared state of the "Automation" area: selection lists (groups, mail rules, accounts), names for sentence and
// cards, AI/email on?, handing over a draft from the template gallery to the editor.
import { listAllOf } from '../../api/admin'
import { data } from '../../stores/data.svelte'
import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
import { aiStatus } from '../ai/status.svelte'
import type { Draft, NameKind, Names } from './model'

interface Named {
  id: number
  name: string
}

class AutomationState {
  groups = $state<Named[]>([])
  mailRules = $state<(Named & Record<string, any>)[]>([])
  mailAccounts = $state<(Named & Record<string, any>)[]>([])
  loaded = $state(false)
  /** Draft from gallery/suggestion/AI → editor (`automation/workflows/new`) */
  pending = $state<{ draft: Draft; note?: string | null } | null>(null)
  private loading: Promise<void> | null = null

  load(force = false): Promise<void> {
    if (this.loading && !force) return this.loading
    const safe = <T>(p: Promise<{ results: T[] }>) => p.then((r) => r.results).catch(() => [] as T[])
    this.loading = Promise.all([
      session.can('view', 'group') ? safe<Named>(listAllOf('groups')) : Promise.resolve([] as Named[]),
      session.can('view', 'mailrule') ? safe<Named & Record<string, any>>(listAllOf('mail_rules')) : Promise.resolve([]),
      session.can('view', 'mailaccount') ? safe<Named & Record<string, any>>(listAllOf('mail_accounts')) : Promise.resolve([]),
    ]).then(([g, r, a]) => {
      this.groups = g
      this.mailRules = r
      this.mailAccounts = a
      this.loaded = true
    })
    return this.loading
  }

  get aiOn(): boolean {
    return !!(aiStatus.enabled || session.get(SETTINGS_KEYS.AI_ENABLED))
  }
  get emailOn(): boolean {
    return !!session.get(SETTINGS_KEYS.EMAIL_ENABLED)
  }

  /** Names from the loaded master data (null = unknown/not visible) */
  names: Names = (kind: NameKind, id: number) => {
    switch (kind) {
      case 'tag':
        return data.tags.get(id)?.name ?? null
      case 'correspondent':
        return data.correspondents.get(id)?.name ?? null
      case 'document_type':
        return data.documentTypes.get(id)?.name ?? null
      case 'storage_path':
        return data.storagePaths.get(id)?.name ?? null
      case 'user':
        return data.users.get(id)?.username ?? null
      case 'group':
        return this.groups.find((g) => g.id === id)?.name ?? null
      case 'custom_field':
        return data.customFields.find((f) => f.id === id)?.name ?? null
      case 'mail_rule':
        return this.mailRules.find((r) => r.id === id)?.name ?? null
    }
  }

  options(kind: NameKind): Named[] {
    switch (kind) {
      case 'tag':
        return [...data.tags.values()].map((x) => ({ id: x.id, name: x.name, color: x.color })) as Named[]
      case 'correspondent':
        return [...data.correspondents.values()]
      case 'document_type':
        return [...data.documentTypes.values()]
      case 'storage_path':
        return [...data.storagePaths.values()]
      case 'user':
        return [...data.users.values()].map((u) => ({ id: u.id, name: u.username }))
      case 'group':
        return this.groups
      case 'custom_field':
        return data.customFields.map((f) => ({ id: f.id, name: f.name }))
      case 'mail_rule':
        return this.mailRules
    }
  }
}

export const auto = new AutomationState()
