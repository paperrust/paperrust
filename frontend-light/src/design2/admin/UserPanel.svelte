<!-- Edit/create user (docs/DESIGN2-ENTWIRREN.md §2): person / login / access. Saved via v1
     (/api/users/, like UserEditDialog), role hint via POST /rs/api/v2/roles/{id}/assign. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { createOf, deleteOf, patchOf } from '../../api/admin'
  import { ApiError } from '../../api/v1'
  import Icon from '../components/Icon.svelte'
  import Panel from './Panel.svelte'
  import RolePicker from './RolePicker.svelte'
  import TopicSwitches from './TopicSwitches.svelte'
  import { assignRole, errorText, RolesError } from '../api/roles'
  import {
    applyModelSwitch,
    applySwitch,
    classify,
    effectivePerms,
    pickableRoles,
    rolePerms,
    samePerms,
    summarize,
    visibleTopics,
    withoutAlways,
    type Action,
    type GroupLike,
    type RolesInfo,
  } from './roles'
  import { roleName, summarySentence } from './labels'

  let {
    user,
    groups,
    info,
    meId,
    meSuper,
    canChange,
    canDelete,
    startDelete = false,
    hint = null,
    onclose,
    onsaved,
    ondeleted,
  }: {
    user: any | null
    groups: (GroupLike & { name: string })[]
    info: RolesInfo
    meId: number | null
    meSuper: boolean
    canChange: boolean
    canDelete: boolean
    startDelete?: boolean
    hint?: number | string | null
    onclose: () => void
    onsaved: (u: any, msg: string) => void
    ondeleted: (id: number, msg: string) => void
  } = $props()

  let panel: Panel
  export function requestClose(then?: () => void) {
    panel.requestClose(then)
  }

  // Panel is recreated per selection ({#key}): take over props once here
  const S = (() => {
    const isNew = !user
    const pickable = pickableRoles(info, false, meSuper)
    const defaultRole = isNew ? (pickable.find((r) => r.key === 'read') ?? pickable.find((r) => !r.admin)) : null
    const start = {
      username: user?.username ?? '',
      first_name: user?.first_name ?? '',
      last_name: user?.last_name ?? '',
      email: user?.email ?? '',
      is_active: user ? !!user.is_active : true,
      groups: [...(user?.groups ?? [])] as number[],
      perms: defaultRole ? rolePerms(defaultRole, info.always) : ([...(user?.user_permissions ?? [])] as string[]).sort(),
      superuser: !!user?.is_superuser,
      password: null as string | null,
    }
    const isSelf = !!user && user.id === meId
    return {
      isNew,
      isSelf,
      readonly: isNew ? false : !canChange,
      pickable,
      topics: visibleTopics(info.topics, meSuper),
      start,
      startCls: classify(start.perms, info, { superuser: start.superuser, resolved: hint }),
      confirmDelete: startDelete && !isSelf,
      mfa: !!user?.is_mfa_enabled,
      title: isNew ? t('m.admin.new_user') : [user.first_name, user.last_name].filter(Boolean).join(' ') || user.username,
    }
  })()
  const { isNew, isSelf, readonly, pickable, topics, start, startCls, title } = S
  let form = $state(JSON.parse(JSON.stringify(start)) as typeof start)
  const dirty = $derived(JSON.stringify(form) !== JSON.stringify(start))

  // unchanged → server detection (resolve), otherwise local
  const cls = $derived(form.superuser === start.superuser && samePerms(form.perms, start.perms, info.always) ? startCls : classify(form.perms, info, { superuser: form.superuser }))
  /** Role for "Back to role X": last chosen or detected on open */
  let lastRole = $state(startCls.kind === 'role' ? startCls.role : null)
  const pickValue = $derived(cls.kind === 'role' ? cls.role.id : cls.kind === 'none' ? 'none' : null)
  let fineOpen = $state(startCls.kind === 'individual')
  let busy = $state(false)
  let error = $state<string | null>(null)
  let confirmDelete = $state(S.confirmDelete)
  let confirmTfa = $state(false)
  let mfa = $state(S.mfa)
  let pwOpen = $state(isNew)

  function pick(v: number | string) {
    error = null
    if (v === 'none') {
      form.superuser = false
      form.perms = info.always.map((p) => p.replace(/^\w+\./, '')).sort()
      lastRole = null
      return
    }
    const r = info.roles.find((x) => String(x.id) === String(v))
    if (!r) return
    lastRole = r
    if (r.admin) form.superuser = true
    else {
      form.superuser = false
      form.perms = rolePerms(r, info.always)
    }
  }
  function toggle(topic: string, action: Action, on: boolean) {
    form.perms = applySwitch(form.perms, info.topics, topic, action, on)
  }
  function toggleModel(model: string, action: Action, on: boolean) {
    form.perms = applyModelSwitch(form.perms, model, action, on)
  }

  const groupName = (id: number) => groups.find((g) => g.id === id)?.name ?? `#${id}`
  const freeGroups = $derived(groups.filter((g) => !form.groups.includes(g.id)))
  const effective = $derived(summarize(effectivePerms({ id: 0, groups: form.groups, user_permissions: form.perms }, groups), topics, form.superuser))

  const selfDemotion = $derived(isSelf && start.superuser && !form.superuser)

  function failText(e: unknown): string {
    if (e instanceof RolesError) return e.message
    if (e instanceof ApiError) return errorText(e.body) ?? t('m.admin.error_save')
    return t('m.admin.error_save')
  }

  async function save() {
    error = null
    if (!form.username.trim()) {
      error = t('m.admin.username_required')
      return
    }
    if (selfDemotion) {
      error = t('m.admin.self_admin')
      return
    }
    busy = true
    try {
      const body: Record<string, unknown> = {
        username: form.username.trim(),
        first_name: form.first_name,
        last_name: form.last_name,
        email: form.email,
        is_active: isSelf ? true : form.is_active,
        groups: form.groups,
      }
      if (form.password) body.password = form.password
      // Role chosen and role API available: permissions and superuser are written by POST /roles/{id}/assign (only permissions
      // existing in the DB, protection against self-demotion); otherwise as before via v1.
      const roleChanged = isNew || cls.kind !== startCls.kind || (cls.kind === 'role' && startCls.kind === 'role' && cls.role.id !== startCls.role.id)
      const viaRole = !info.fallback && cls.kind === 'role' && roleChanged
      const permsChanged = isNew || !samePerms(form.perms, start.perms)
      if (permsChanged && !viaRole) body.user_permissions = form.perms
      if (!viaRole && meSuper && (isNew || form.superuser !== start.superuser)) {
        body.is_superuser = form.superuser
        if (form.superuser) body.is_staff = true
      }
      const saved: any = isNew ? await createOf<any>('users', body) : await patchOf<any>('users', { id: user.id, ...body })
      // Assign role: writes the permissions (administrator → superuser) and the hint
      if (viaRole && cls.kind === 'role') {
        await assignRole(cls.role.id, { users: [saved.id] })
      }
      if (isSelf && form.password) {
        onsaved(saved, t('admin.users.password_changed_logged_out'))
        setTimeout(() => (location.href = `${location.origin}/accounts/logout/?next=/accounts/login/?next=/`), 2500)
        return
      }
      onsaved(saved, t('m.admin.saved_user', { PH: saved.username ?? form.username }))
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
      await deleteOf('users', user.id)
      ondeleted(user.id, t('m.admin.deleted_user', { PH: user.username }))
    } catch (e) {
      error = failText(e)
      confirmDelete = false
    } finally {
      busy = false
    }
  }

  async function resetTfa() {
    busy = true
    error = null
    try {
      const ok = await api.request<boolean>('POST', `/api/users/${user.id}/deactivate_totp/`, null)
      if (ok) mfa = false
      else error = t('dialog.user.totp_deactivation_failed')
    } catch (e) {
      error = failText(e)
    } finally {
      busy = false
      confirmTfa = false
    }
  }
</script>

<Panel bind:this={panel} {title} {dirty} {onclose}>
  <form id="ad-user-form" onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off" novalidate>
    <fieldset disabled={readonly || busy} class="ad-fs">
      <section class="ad-sec" aria-labelledby="ad-u-person">
        <h3 id="ad-u-person">{t('m.admin.sec_person')}</h3>
        <div class="ad-row2">
          <div class="ad-field"><label for="ad-u-first">{t('dialog.user.first_name')}</label><input id="ad-u-first" class="d2-input" bind:value={form.first_name} data-autofocus={isNew || undefined} /></div>
          <div class="ad-field"><label for="ad-u-last">{t('dialog.user.last_name')}</label><input id="ad-u-last" class="d2-input" bind:value={form.last_name} /></div>
        </div>
        <div class="ad-field"><label for="ad-u-mail">{t('common.email')}</label><input id="ad-u-mail" class="d2-input" type="email" bind:value={form.email} /></div>
      </section>

      <section class="ad-sec" aria-labelledby="ad-u-login">
        <h3 id="ad-u-login">{t('m.admin.sec_login')}</h3>
        <div class="ad-field"><label for="ad-u-name">{t('common.username')}</label><input id="ad-u-name" class="d2-input" bind:value={form.username} required autocomplete="off" /></div>
        <div class="ad-field">
          {#if pwOpen}
            <label for="ad-u-pw">{isNew ? t('common.password') : t('m.admin.new_password')}</label>
            <input id="ad-u-pw" class="d2-input" type="password" autocomplete="new-password" value={form.password ?? ''} oninput={(e) => (form.password = e.currentTarget.value || null)} />
            {#if !isNew}<p class="ad-hint">{t('m.admin.password_set_hint')}</p>{/if}
          {:else}
            <span class="ad-lbl">{t('common.password')}</span>
            <button type="button" class="d2-btn sm" onclick={() => (pwOpen = true)}>{t('m.admin.set_password')}</button>
          {/if}
        </div>
        {#if !isNew}
          <div class="ad-field">
            <span class="ad-lbl">{t('dialog.user.two_factor_authentication')}</span>
            <div class="ad-line">
              <span>{mfa ? t('m.admin.tfa_on') : t('m.admin.tfa_off')}</span>
              {#if mfa && meSuper}<button type="button" class="d2-btn sm" onclick={() => (confirmTfa = true)}>{t('m.admin.tfa_reset')}</button>{/if}
            </div>
          </div>
        {/if}
        <div class="ad-field ad-switchrow">
          <span id="ad-u-lock-l">{t('m.admin.account_locked')}<span class="ad-hint ad-block">{isSelf ? t('m.admin.self_lock') : t('m.admin.account_locked_hint')}</span></span>
          <button type="button" role="switch" class="ad-toggle" aria-labelledby="ad-u-lock-l" aria-checked={!form.is_active} disabled={isSelf} onclick={() => (form.is_active = !form.is_active)}></button>
        </div>
      </section>

      <section class="ad-sec" aria-labelledby="ad-u-access">
        <h3 id="ad-u-access">{t('m.admin.sec_access')}</h3>
        <div class="ad-field">
          <span class="ad-lbl" id="ad-u-role-l">{t('m.admin.col_role')}</span>
          <RolePicker roles={pickable} value={pickValue} onpick={pick} allowNone individual={cls.kind === 'individual'} labelledby="ad-u-role-l" name="ad-u-role" disabled={readonly || busy || (isSelf && start.superuser)} />
          {#if cls.kind === 'individual' && lastRole}
            <button type="button" class="d2-btn sm ad-back" onclick={() => pick(lastRole!.id)}>{t('m.admin.back_to_role', { PH: roleName(lastRole) })}</button>
          {/if}
          {#if isSelf && start.superuser}<p class="ad-hint">{t('m.admin.self_admin')}</p>{/if}
        </div>

        <div class="ad-field">
          <span class="ad-lbl" id="ad-u-groups-l">{t('common.groups')}</span>
          {#if form.groups.length}
            <ul class="ad-chips" aria-labelledby="ad-u-groups-l">
              {#each form.groups as gid (gid)}
                <li class="ad-chip">{groupName(gid)}<button type="button" aria-label={t('m.admin.remove_x', { PH: groupName(gid) })} onclick={() => (form.groups = form.groups.filter((x) => x !== gid))}><Icon name="x-lg" size="0.7rem" /></button></li>
              {/each}
            </ul>
          {/if}
          {#if freeGroups.length}
            <select class="d2-select" aria-label={t('m.admin.add_group_to_user')} value="" onchange={(e) => { const v = +e.currentTarget.value; if (v) form.groups = [...form.groups, v]; e.currentTarget.value = '' }}>
              <option value="">{t('m.admin.add_group_to_user')}</option>
              {#each freeGroups as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
            </select>
          {/if}
        </div>

        <button type="button" class="ad-expander" aria-expanded={fineOpen} aria-controls="ad-u-fine" onclick={() => (fineOpen = !fineOpen)}><Icon name="chevron-right" />{t('m.admin.fine_tune')}</button>
        {#if fineOpen}
          <div id="ad-u-fine">
            {#if form.superuser}<p class="ad-hint">{t('m.admin.admin_all_rights')}</p>
            {:else}<TopicSwitches {topics} perms={form.perms} onchange={toggle} onmodel={toggleModel} disabled={readonly || busy} idPrefix="ad-u-sw" />{/if}
            {#if withoutAlways(form.perms, info.always).length && cls.kind === 'individual'}<p class="ad-hint">{t('m.admin.individual_hint')}</p>{/if}
          </div>
        {/if}

        <div class="ad-eff" aria-live="polite">
          <strong>{t('m.admin.effective')}</strong>
          {#if !form.is_active}{t('m.admin.eff_locked')}{:else}{summarySentence(effective)}{/if}
        </div>
      </section>
    </fieldset>
  </form>

  {#snippet confirm()}
    {#if confirmDelete}
      <div class="ad-confirm danger" role="group" aria-labelledby="ad-u-del-q">
        <p id="ad-u-del-q">{t('m.admin.delete_user_q', { PH: user.username })}</p>
        <div class="ad-line">
          <button type="button" class="d2-btn" onclick={() => (confirmDelete = false)} disabled={busy}>{t('common.cancel')}</button>
          <button type="button" class="d2-btn ad-danger-solid" onclick={doDelete} disabled={busy}>{t('common.delete')}</button>
        </div>
      </div>
    {:else if confirmTfa}
      <div class="ad-confirm" role="group" aria-labelledby="ad-u-tfa-q">
        <p id="ad-u-tfa-q">{t('m.admin.tfa_reset_q')}</p>
        <div class="ad-line">
          <button type="button" class="d2-btn" onclick={() => (confirmTfa = false)} disabled={busy}>{t('common.cancel')}</button>
          <button type="button" class="d2-btn ad-danger-solid" onclick={resetTfa} disabled={busy}>{t('m.admin.tfa_reset')}</button>
        </div>
      </div>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if error}<p class="ad-err ad-full" role="alert">{error}</p>{/if}
    {#if !isNew && canDelete && !isSelf}
      <button type="button" class="d2-btn ghost danger" onclick={() => (confirmDelete = true)} disabled={busy}><Icon name="trash" />{t('common.delete')}</button>
    {/if}
    <span class="ad-grow"></span>
    <button type="button" class="d2-btn" onclick={() => panel.requestClose()} disabled={busy}>{t('common.cancel')}</button>
    {#if !readonly}<button type="submit" form="ad-user-form" class="d2-btn primary" disabled={busy || (!dirty && !isNew)}>{t('common.save')}</button>{/if}
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
