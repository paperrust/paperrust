<!-- Edit/create group (docs/DESIGN2-ENTWIRREN.md §2): name, role, members, fine-tuning.
     Saved via v1 (/api/groups/ {name, permissions}; members = `groups` of the users), role hint via
     POST /rs/api/v2/roles/{id}/assign. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { createOf, deleteOf, patchOf } from '../../api/admin'
  import { ApiError } from '../../api/v1'
  import Icon from '../components/Icon.svelte'
  import Panel from './Panel.svelte'
  import RolePicker from './RolePicker.svelte'
  import TopicSwitches from './TopicSwitches.svelte'
  import { assignRole, errorText, RolesError } from '../api/roles'
  import { applyModelSwitch, applySwitch, classify, pickableRoles, rolePerms, samePerms, summarize, visibleTopics, type Action, type RolesInfo } from './roles'
  import { roleName, summarySentence } from './labels'

  let {
    group,
    users,
    info,
    meSuper,
    canChange,
    canDelete,
    canChangeUsers,
    startDelete = false,
    hint = null,
    onclose,
    onsaved,
    ondeleted,
  }: {
    group: any | null
    users: any[]
    info: RolesInfo
    meSuper: boolean
    canChange: boolean
    canDelete: boolean
    canChangeUsers: boolean
    startDelete?: boolean
    hint?: number | string | null
    onclose: () => void
    onsaved: (g: any, msg: string) => void
    ondeleted: (id: number, msg: string) => void
  } = $props()

  let panel: Panel
  export function requestClose(then?: () => void) {
    panel.requestClose(then)
  }

  // Panel is recreated per selection ({#key}): take over props once here
  const S = (() => {
    const isNew = !group
    const pickable = pickableRoles(info, true, meSuper)
    const defaultRole = isNew ? (pickable.find((r) => r.key === 'read') ?? pickable[0]) : null
    const start = {
      name: group?.name ?? '',
      perms: defaultRole ? rolePerms(defaultRole, info.always) : ([...(group?.permissions ?? [])] as string[]).sort(),
      members: (group ? users.filter((u) => (u.groups ?? []).includes(group.id)).map((u) => u.id as number) : []).sort((a, b) => a - b),
    }
    return {
      isNew,
      readonly: isNew ? false : !canChange,
      pickable,
      topics: visibleTopics(info.topics, meSuper),
      start,
      startCls: classify(start.perms, info, { resolved: hint }),
      confirmDelete: startDelete,
      title: isNew ? t('m.admin.new_group') : group.name,
    }
  })()
  const { isNew, readonly, pickable, topics, start, startCls, title } = S
  let form = $state(JSON.parse(JSON.stringify(start)) as typeof start)
  const dirty = $derived(JSON.stringify(form) !== JSON.stringify(start))
  const cls = $derived(samePerms(form.perms, start.perms, info.always) ? startCls : classify(form.perms, info))
  let lastRole = $state(startCls.kind === 'role' ? startCls.role : null)
  const pickValue = $derived(cls.kind === 'role' ? cls.role.id : cls.kind === 'none' ? 'none' : null)
  let fineOpen = $state(startCls.kind === 'individual')
  let busy = $state(false)
  let error = $state<string | null>(null)
  let confirmDelete = $state(S.confirmDelete)

  function pick(v: number | string) {
    const r = info.roles.find((x) => String(x.id) === String(v))
    if (!r) return
    lastRole = r
    form.perms = rolePerms(r, info.always)
  }
  const toggle = (topic: string, action: Action, on: boolean) => (form.perms = applySwitch(form.perms, info.topics, topic, action, on))
  const toggleModel = (model: string, action: Action, on: boolean) => (form.perms = applyModelSwitch(form.perms, model, action, on))
  const userLabel = (u: any) => [u.first_name, u.last_name].filter(Boolean).join(' ') || u.username
  const byId = (id: number) => users.find((u) => u.id === id)
  const freeUsers = $derived(users.filter((u) => !form.members.includes(u.id)))
  const effective = $derived(summarize(form.perms, topics))

  function failText(e: unknown): string {
    if (e instanceof RolesError) return e.message
    if (e instanceof ApiError) return errorText(e.body) ?? t('m.admin.error_save')
    return t('m.admin.error_save')
  }

  async function save() {
    error = null
    if (!form.name.trim()) {
      error = t('m.admin.name_required')
      return
    }
    busy = true
    try {
      const body: Record<string, unknown> = { name: form.name.trim() }
      const roleChanged = isNew || cls.kind !== startCls.kind || (cls.kind === 'role' && startCls.kind === 'role' && cls.role.id !== startCls.role.id)
      const viaRole = !info.fallback && cls.kind === 'role' && roleChanged
      if (!viaRole && (isNew || !samePerms(form.perms, start.perms))) body.permissions = form.perms
      const saved: any = isNew ? await createOf<any>('groups', body) : await patchOf<any>('groups', { id: group.id, ...body })
      if (viaRole && cls.kind === 'role') {
        await assignRole(cls.role.id, { groups: [saved.id] })
      }
      // Members: adjust `groups` of the affected users
      if (canChangeUsers) {
        const added = form.members.filter((id) => !start.members.includes(id))
        const removed = start.members.filter((id) => !form.members.includes(id))
        for (const id of [...added, ...removed]) {
          const u = byId(id)
          if (!u) continue
          const gs: number[] = added.includes(id) ? [...new Set([...(u.groups ?? []), saved.id])] : (u.groups ?? []).filter((g: number) => g !== saved.id)
          await patchOf<any>('users', { id, groups: gs })
        }
      }
      onsaved(saved, t('m.admin.saved_group', { PH: saved.name ?? form.name }))
    } catch (e) {
      error = failText(e)
    } finally {
      busy = false
    }
  }

  async function doDelete() {
    busy = true
    error = null
    try {
      await deleteOf('groups', group.id)
      ondeleted(group.id, t('m.admin.deleted_group', { PH: group.name }))
    } catch (e) {
      error = failText(e)
      confirmDelete = false
    } finally {
      busy = false
    }
  }
</script>

<Panel bind:this={panel} {title} {dirty} {onclose}>
  <form id="ad-group-form" onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off" novalidate>
    <fieldset disabled={readonly || busy} class="ad-fs">
      <section class="ad-sec">
        <div class="ad-field"><label for="ad-g-name">{t('common.name')}</label><input id="ad-g-name" class="d2-input" bind:value={form.name} required data-autofocus={isNew || undefined} /></div>
      </section>

      <section class="ad-sec" aria-labelledby="ad-g-access">
        <h3 id="ad-g-access">{t('m.admin.sec_access')}</h3>
        <div class="ad-field">
          <span class="ad-lbl" id="ad-g-role-l">{t('m.admin.col_role')}</span>
          <RolePicker roles={pickable} value={pickValue} onpick={pick} individual={cls.kind === 'individual'} labelledby="ad-g-role-l" name="ad-g-role" disabled={readonly || busy} />
          {#if cls.kind === 'none'}<p class="ad-hint">{t('m.admin.group_no_rights')}</p>{/if}
          {#if cls.kind === 'individual' && lastRole}
            <button type="button" class="d2-btn sm ad-back" onclick={() => pick(lastRole!.id)}>{t('m.admin.back_to_role', { PH: roleName(lastRole) })}</button>
          {/if}
        </div>
        <button type="button" class="ad-expander" aria-expanded={fineOpen} aria-controls="ad-g-fine" onclick={() => (fineOpen = !fineOpen)}><Icon name="chevron-right" />{t('m.admin.fine_tune')}</button>
        {#if fineOpen}
          <div id="ad-g-fine"><TopicSwitches {topics} perms={form.perms} onchange={toggle} onmodel={toggleModel} disabled={readonly || busy} idPrefix="ad-g-sw" /></div>
        {/if}
        <div class="ad-eff" aria-live="polite"><strong>{t('m.admin.effective')}</strong>{summarySentence(effective)}</div>
      </section>

      <section class="ad-sec" aria-labelledby="ad-g-members">
        <h3 id="ad-g-members">{t('m.admin.col_members')} ({form.members.length})</h3>
        {#if form.members.length}
          <ul class="ad-chips">
            {#each form.members as id (id)}
              {@const u = byId(id)}
              <li class="ad-chip">{u ? userLabel(u) : `#${id}`}{#if canChangeUsers}<button type="button" aria-label={t('m.admin.remove_x', { PH: u ? userLabel(u) : String(id) })} onclick={() => (form.members = form.members.filter((x) => x !== id))}><Icon name="x-lg" size="0.7rem" /></button>{/if}</li>
            {/each}
          </ul>
        {:else}
          <p class="ad-hint">{t('m.admin.no_members')}</p>
        {/if}
        {#if canChangeUsers && freeUsers.length}
          <select class="d2-select" aria-label={t('m.admin.add_member')} value="" onchange={(e) => { const v = +e.currentTarget.value; if (v) form.members = [...form.members, v]; e.currentTarget.value = '' }}>
            <option value="">{t('m.admin.add_member')}</option>
            {#each freeUsers as u (u.id)}<option value={u.id}>{userLabel(u)} ({u.username})</option>{/each}
          </select>
        {/if}
      </section>
    </fieldset>
  </form>

  {#snippet confirm()}
    {#if confirmDelete}
      <div class="ad-confirm danger" role="group" aria-labelledby="ad-g-del-q">
        <p id="ad-g-del-q">{t('m.admin.delete_group_q', { PH: group.name })}</p>
        <div class="ad-line">
          <button type="button" class="d2-btn" onclick={() => (confirmDelete = false)} disabled={busy}>{t('common.cancel')}</button>
          <button type="button" class="d2-btn ad-danger-solid" onclick={doDelete} disabled={busy}>{t('common.delete')}</button>
        </div>
      </div>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if error}<p class="ad-err ad-full" role="alert">{error}</p>{/if}
    {#if !isNew && canDelete}
      <button type="button" class="d2-btn ghost danger" onclick={() => (confirmDelete = true)} disabled={busy}><Icon name="trash" />{t('common.delete')}</button>
    {/if}
    <span class="ad-grow"></span>
    <button type="button" class="d2-btn" onclick={() => panel.requestClose()} disabled={busy}>{t('common.cancel')}</button>
    {#if !readonly}<button type="submit" form="ad-group-form" class="d2-btn primary" disabled={busy || (!dirty && !isNew)}>{t('common.save')}</button>{/if}
  {/snippet}
</Panel>

<style>
  .ad-fs {
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
  :global(.ad-back) {
    margin-top: var(--d2-s2);
  }
</style>
