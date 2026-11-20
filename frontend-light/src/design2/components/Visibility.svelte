<!-- "Visible to" (docs/DESIGN2-ENTWIRREN.md §3): one field with Everyone · My team · Only me · Specific people …
     Mapping to owner + set_permissions in lib/visibility.ts (header comment). Opening never changes anything:
     the detected quick choice is shown, writing happens only on a choice.
     Single document (default): quick choice is reported immediately (onchange), "Specific people" opens the detailed
     selection with "Apply". `deferred` (bulk edit): no initial value, every change goes as a draft to
     ondraft, the caller applies. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { data } from '../../stores/data.svelte'
  import { session } from '../../stores/settings.svelte'
  import { groups } from '../../components/common/input/permissions/permissions-group/groups.svelte'
  import { apply, available, classify, effectiveOwner, emptyPerms, normPerms, summarize, type Perms, type VisibilityMode, type VisibilityValue } from '../lib/visibility'
  import ObjectPicker from './ObjectPicker.svelte'

  let {
    owner = null,
    permissions = undefined,
    disabled = false,
    deferred = false,
    id = 'd2-visibility',
    onchange = undefined,
    ondraft = undefined,
  }: {
    owner?: number | null
    /** `permissions` from full_perms=true; undefined = unknown (field read-only) */
    permissions?: Perms | undefined
    disabled?: boolean
    deferred?: boolean
    id?: string
    onchange?: (v: VisibilityValue) => void
    ondraft?: (v: VisibilityValue, mode: VisibilityMode) => void
  } = $props()

  const meUser = $derived(session.data?.user as { id: number; groups?: number[] } | undefined)
  const me = $derived(meUser?.id ?? 0)
  const known = $derived(deferred || permissions !== undefined)
  const ctxOwner = $derived(deferred ? null : owner)
  const effOwner = $derived(effectiveOwner({ owner: ctxOwner, me }))
  /** Groups of a user: own ones from the session, otherwise from the user list (null = unknown) */
  function groupsOf(uid: number): number[] | null {
    if (uid === me && meUser?.groups) return meUser.groups
    return data.users.get(uid)?.groups ?? null
  }
  const ownerGroups = $derived(groupsOf(effOwner))
  const current = $derived<VisibilityMode | ''>(deferred ? '' : known ? classify(owner, permissions, owner === null ? null : groupsOf(owner)) : '')

  let editing = $state(false)
  let picked = $state<VisibilityMode | ''>('')
  let draft = $state<VisibilityValue>({ owner: null, set_permissions: emptyPerms() })
  const shown = $derived(editing ? 'custom' : deferred ? picked : current)

  $effect(() => {
    if (editing || shown === 'custom' || shown === 'team') void groups.load()
  })

  function userName(uid: number): string {
    const u = uid === me ? (session.data?.user as { username?: string; first_name?: string; last_name?: string } | undefined) : data.users.get(uid)
    if (!u) return '#' + uid
    const full = [u.first_name, u.last_name].filter(Boolean).join(' ').trim()
    return full || u.username || '#' + uid
  }
  const groupName = (gid: number) => groups.list.find((g) => g.id === gid)?.name ?? '#' + gid
  const labels = $derived({ owner: t('m.visibility.owner'), noOwner: t('m.visibility.no_owner'), view: t('m.visibility.can_view'), change: t('m.visibility.can_change'), nobody: t('m.visibility.nobody') })
  const privateLabel = $derived(!deferred && owner !== null && owner !== me ? t('m.visibility.private_owner', { PH: userName(owner) }) : t('m.visibility.private'))

  const hint = $derived.by(() => {
    if (!known) return ''
    const m = shown
    if (m === 'all') return t('m.visibility.hint_all')
    if (m === 'team') return t('m.visibility.hint_team', { PH: (ownerGroups ?? []).map(groupName).join(', ') })
    if (m === 'private') return t('m.visibility.hint_private')
    if (m === 'custom' && !editing) return summarize(owner, permissions, userName, groupName, labels)
    return ''
  })

  function pick(m: VisibilityMode) {
    if (m === 'custom') {
      draft = deferred
        ? { owner: me || null, set_permissions: emptyPerms() }
        : { owner, set_permissions: normPerms(permissions) }
      editing = true
      picked = 'custom'
      if (deferred) ondraft?.(snapshot(), 'custom')
      return
    }
    editing = false
    const v = apply(m, { owner: ctxOwner, me, ownerGroups })
    if (!v) return
    if (deferred) {
      picked = m
      ondraft?.(v, m)
    } else onchange?.(v)
  }
  const snapshot = (): VisibilityValue => ({ owner: draft.owner, set_permissions: normPerms(draft.set_permissions) })
  function edit(fn: (d: VisibilityValue) => void) {
    const d = snapshot()
    fn(d)
    draft = d
    if (deferred) ondraft?.(snapshot(), 'custom')
  }
  function commit() {
    editing = false
    onchange?.(snapshot())
  }
  function cancel() {
    editing = false
  }

  const userOpts = $derived([...data.users.values()].map((u) => ({ id: u.id, name: userName(u.id) })).sort((a, b) => a.name.localeCompare(b.name)))
  const groupOpts = $derived(groups.list.map((g) => ({ id: g.id, name: g.name })).sort((a, b) => a.name.localeCompare(b.name)))
  const nums = (n: (number | null)[]) => n.filter((x): x is number => x !== null)
</script>

<div class="vis" {id}>
  <select class="d2-select" id="{id}-mode" aria-label={t('m.visibility.label')} aria-describedby="{id}-hint" value={shown} disabled={disabled || !known}
    onchange={(e) => {
      const el = e.currentTarget
      pick(el.value as VisibilityMode)
      // Selection follows the actual state (e.g. when nothing was applied)
      el.value = shown
    }}>
    {#if shown === ''}<option value="" disabled>{known ? t('m.visibility.choose') : '–'}</option>{/if}
    <option value="all">{t('m.visibility.all')}</option>
    <option value="team" disabled={!available('team', ownerGroups)}>{available('team', ownerGroups) ? t('m.visibility.team') : t('m.visibility.team_none')}</option>
    <option value="private">{privateLabel}</option>
    <option value="custom">{t('m.visibility.custom')}</option>
  </select>
  {#if hint}<p class="hint" id="{id}-hint">{hint}</p>{/if}

  {#if editing}
    <fieldset class="exact" {disabled}>
      <legend class="d2-sr">{t('m.visibility.custom')}</legend>
      <div class="f">
        <span class="d2-label">{t('m.visibility.owner')}</span>
        <ObjectPicker label={t('m.visibility.owner')} id="{id}-owner" options={userOpts} allowNone noneLabel={t('m.visibility.no_owner')} selected={[draft.owner]} placeholder={t('m.visibility.no_owner')}
          onchange={(n) => edit((d) => (d.owner = (n[0] as number | null | undefined) ?? null))} />
      </div>
      <h4>{t('m.visibility.can_view')}</h4>
      <div class="two">
        <ObjectPicker label={t('m.visibility.users')} id="{id}-view-users" multi options={userOpts} selected={draft.set_permissions.view.users} placeholder={t('m.visibility.users')}
          onchange={(n) => edit((d) => (d.set_permissions.view.users = nums(n)))} />
        <ObjectPicker label={t('m.visibility.groups')} id="{id}-view-groups" multi options={groupOpts} selected={draft.set_permissions.view.groups} placeholder={t('m.visibility.groups')}
          onchange={(n) => edit((d) => (d.set_permissions.view.groups = nums(n)))} />
      </div>
      <h4>{t('m.visibility.can_change')}</h4>
      <div class="two">
        <ObjectPicker label={t('m.visibility.users')} id="{id}-change-users" multi options={userOpts} selected={draft.set_permissions.change.users} placeholder={t('m.visibility.users')}
          onchange={(n) => edit((d) => (d.set_permissions.change.users = nums(n)))} />
        <ObjectPicker label={t('m.visibility.groups')} id="{id}-change-groups" multi options={groupOpts} selected={draft.set_permissions.change.groups} placeholder={t('m.visibility.groups')}
          onchange={(n) => edit((d) => (d.set_permissions.change.groups = nums(n)))} />
      </div>
      <p class="hint">{t('m.visibility.edit_implies_view')}{draft.owner === null ? ' ' + t('m.visibility.hint_all') : ''}</p>
      {#if !deferred}
        <div class="acts">
          <button type="button" class="d2-btn sm" onclick={cancel}>{t('common.cancel')}</button>
          <button type="button" class="d2-btn sm primary" id="{id}-apply" onclick={commit}>{t('m.visibility.apply')}</button>
        </div>
      {/if}
    </fieldset>
  {/if}
</div>

<style>
  .vis {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .hint {
    margin: 0;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    overflow-wrap: anywhere;
  }
  .exact {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    margin: var(--d2-s2) 0 0;
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
    min-width: 0;
  }
  .exact h4 {
    margin: var(--d2-s1) 0 0;
    font-size: var(--d2-fs-sm);
    font-weight: 600;
  }
  .f {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--d2-s2);
  }
  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }
</style>
