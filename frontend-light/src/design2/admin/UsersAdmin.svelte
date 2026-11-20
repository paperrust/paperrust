<!-- Manage → System → Users and groups in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §2). Table with tabs
     Users | Groups and search; row = sidebar on the right (list stays visible). Data via v1 /api/users/ and
     /api/groups/ (like the classic page UsersGroups), roles via /rs/api/v2/roles (otherwise local default). -->
<script lang="ts">
  import './admin.css'
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { listAllOf, patchOf } from '../../api/admin'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'
  import { onCommand } from '../lib/bus'
  import { loadRoles, resolveRoles, type Resolved } from '../api/roles'
  import { classify, userAccess, type RolesInfo } from './roles'
  import { accessLabel, roleName } from './labels'
  import UserPanel from './UserPanel.svelte'
  import GroupPanel from './GroupPanel.svelte'

  type Tab = 'users' | 'groups'
  type Sel = { kind: Tab; id: number | null; del?: boolean; n: number }

  const me: any = session.data?.user ?? null
  const meId: number | null = me?.id ?? null
  const meSuper = !!me?.is_superuser
  const canViewUsers = session.can('view', 'user')
  const canViewGroups = session.can('view', 'group')

  let tab = $state<Tab>(canViewUsers || !canViewGroups ? 'users' : 'groups')
  let query = $state('')
  let users = $state<any[] | null>(null)
  let groups = $state<any[] | null>(null)
  let info = $state<RolesInfo | null>(null)
  let hints = $state<Resolved>({ users: {}, groups: {} })
  let failed = $state(false)
  let sel = $state<Sel | null>(null)
  let menuFor = $state<string | null>(null)
  let panelRef = $state<{ requestClose: (then?: () => void) => void } | null>(null)
  let seq = 0

  async function load() {
    failed = false
    try {
      const [u, g, i] = await Promise.all([
        canViewUsers ? listAllOf<any>('users').then((r) => r.results) : Promise.resolve([]),
        canViewGroups ? listAllOf<any>('groups').then((r) => r.results) : Promise.resolve([]),
        loadRoles(),
      ])
      users = u
      groups = g
      info = i
      if (!i.fallback) resolveRoles(u.map((x: any) => x.id), g.map((x: any) => x.id)).then((r) => (hints = r))
    } catch {
      failed = true
    }
  }
  onMount(() => {
    load()
    return onCommand((cmd) => {
      if (cmd === 'escape' && sel && panelRef) {
        panelRef.requestClose()
        return true
      }
      return false
    })
  })

  const norm = (s: unknown) => String(s ?? '').toLocaleLowerCase()
  const fullName = (u: any) => [u.first_name, u.last_name].filter(Boolean).join(' ')
  const groupName = (id: number) => groups?.find((g) => g.id === id)?.name ?? ''
  const shownUsers = $derived.by(() => {
    const q = norm(query.trim())
    const list = users ?? []
    if (!q) return list
    return list.filter((u) => [u.username, u.first_name, u.last_name, u.email, ...(u.groups ?? []).map(groupName)].some((x) => norm(x).includes(q)))
  })
  const shownGroups = $derived.by(() => {
    const q = norm(query.trim())
    return (groups ?? []).filter((g) => !q || norm(g.name).includes(q))
  })
  const memberCount = (gid: number) => (users ?? []).filter((u) => (u.groups ?? []).includes(gid)).length

  function userRole(u: any): { label: string; from: string | null; muted: boolean } {
    if (!info) return { label: '', from: null, muted: true }
    const a = userAccess(u, groups ?? [], info, hints.users[String(u.id)] ?? null, hints.groups)
    return { label: accessLabel(a), from: a.inherited ? a.inherited.name : null, muted: a.kind !== 'role' }
  }
  function groupRole(g: any): { label: string; muted: boolean } {
    if (!info) return { label: '', muted: true }
    const c = classify(g.permissions ?? [], info, { resolved: hints.groups[String(g.id)] ?? null })
    return { label: c.kind === 'role' ? roleName(c.role) : c.kind === 'individual' ? t('m.admin.role_individual') : t('m.admin.role_none'), muted: c.kind !== 'role' }
  }
  function lastLogin(u: any): string {
    if (!('last_login' in u)) return '–'
    if (!u.last_login) return t('m.admin.never')
    const d = new Date(u.last_login)
    return isNaN(+d) ? '–' : d.toLocaleDateString(document.documentElement.lang || undefined, { day: '2-digit', month: '2-digit', year: 'numeric' })
  }

  /** Open panel; an open panel with changes asks first */
  function open(kind: Tab, id: number | null, del = false) {
    menuFor = null
    const go = () => (sel = { kind, id, del, n: ++seq })
    if (sel && panelRef && !(sel.kind === kind && sel.id === id && !del)) panelRef.requestClose(go)
    else if (!sel || del) go()
  }
  const close = () => (sel = null)

  function setTab(next: Tab) {
    if (tab === next) return
    const go = () => {
      tab = next
      query = ''
    }
    if (sel && panelRef) panelRef.requestClose(go)
    else go()
  }
  function tabKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
    e.preventDefault()
    const next: Tab = tab === 'users' ? 'groups' : 'users'
    if ((next === 'users' && canViewUsers) || (next === 'groups' && canViewGroups)) {
      setTab(next)
      queueMicrotask(() => document.getElementById(`ad-tab-${next}`)?.focus())
    }
  }

  function savedUser(u: any, msg: string) {
    toastService.showSuccess(msg)
    sel = null
    load()
    void u
  }
  function deleted(_id: number, msg: string) {
    toastService.showSuccess(msg)
    sel = null
    load()
  }
  async function setActive(u: any, active: boolean) {
    menuFor = null
    try {
      await patchOf<any>('users', { id: u.id, is_active: active })
      toastService.showSuccess(t(active ? 'm.admin.unlocked_user' : 'm.admin.locked_user', { PH: u.username }))
      load()
    } catch (e) {
      toastService.showError(t('m.admin.error_save'), e)
    }
  }
</script>

<div class="ad-page" class:with-panel={!!sel}>
  <div class="ad-main">
    <div class="ad-head">
      <h1>{t('common.users_groups')}</h1>
      {#if meSuper}<a class="d2-btn" href={router.href('roles')} onclick={(e) => { e.preventDefault(); router.navigate('roles') }}><Icon name="person-badge" />{t('m.admin.roles')}</a>{/if}
      {#if tab === 'users' && session.can('add', 'user')}
        <button type="button" class="d2-btn primary" onclick={() => open('users', null)}><Icon name="plus-lg" />{t('m.admin.add_user')}</button>
      {:else if tab === 'groups' && session.can('add', 'group')}
        <button type="button" class="d2-btn primary" onclick={() => open('groups', null)}><Icon name="plus-lg" />{t('m.admin.add_group')}</button>
      {/if}
    </div>
    <div class="ad-tools">
      <div class="ad-tabs" role="tablist" aria-label={t('common.users_groups')}>
        {#if canViewUsers}<button type="button" role="tab" id="ad-tab-users" class="ad-tab" aria-selected={tab === 'users'} aria-controls="ad-list" tabindex={tab === 'users' ? 0 : -1} onclick={() => setTab('users')} onkeydown={tabKey}>{t('common.users')}</button>{/if}
        {#if canViewGroups}<button type="button" role="tab" id="ad-tab-groups" class="ad-tab" aria-selected={tab === 'groups'} aria-controls="ad-list" tabindex={tab === 'groups' ? 0 : -1} onclick={() => setTab('groups')} onkeydown={tabKey}>{t('common.groups')}</button>{/if}
      </div>
      <div class="ad-search">
        <Icon name="search" />
        <input class="d2-input" type="search" bind:value={query} placeholder={tab === 'users' ? t('m.admin.search_users') : t('m.admin.search_groups')} aria-label={tab === 'users' ? t('m.admin.search_users') : t('m.admin.search_groups')} />
      </div>
    </div>
    {#if info?.fallback && meSuper}<p class="ad-note">{t('m.admin.fallback_hint')}</p>{/if}

    <div class="d2-scroll ad-list" id="ad-list" role="tabpanel" aria-labelledby="ad-tab-{tab}">
      {#if failed}
        <div class="d2-empty"><p>{t('m.admin.error_load')}</p><button type="button" class="d2-btn" onclick={load}>{t('common.retry')}</button></div>
      {:else if !users || !groups || !info}
        <div class="d2-empty" aria-busy="true">{t('common.loading')}</div>
      {:else if tab === 'users'}
        {#if !shownUsers.length}
          <div class="d2-empty"><p>{t('m.admin.no_users')}</p></div>
        {:else}
          <table class="ad-table">
            <thead>
              <tr>
                <th scope="col">{t('common.name')}</th>
                <th scope="col">{t('m.admin.col_role')}</th>
                <th scope="col" class="ad-wide">{t('common.groups')}</th>
                <th scope="col" class="ad-wide">{t('m.admin.col_last_login')}</th>
                <th scope="col" class="ad-col-actions"><span class="d2-sr">{t('common.actions')}</span></th>
              </tr>
            </thead>
            <tbody>
              {#each shownUsers as u (u.id)}
                {@const r = userRole(u)}
                <tr class:on={sel?.kind === 'users' && sel.id === u.id} onclick={() => open('users', u.id)}>
                  <td>
                    <button type="button" class="ad-rowbtn" onclick={(e) => { e.stopPropagation(); open('users', u.id) }}>{fullName(u) || u.username}</button>
                    <span class="ad-sub">{u.username}{#if !u.is_active}<span class="ad-lock">{t('m.admin.locked')}</span>{/if}</span>
                  </td>
                  <td>
                    <span class:ad-muted={r.muted}>{r.label}</span>
                    {#if r.from}<span class="ad-sub">{t('m.admin.from_group', { PH: r.from })}</span>{/if}
                  </td>
                  <td class="ad-wide">{(u.groups ?? []).map(groupName).filter(Boolean).join(', ')}</td>
                  <td class="ad-wide ad-muted">{lastLogin(u)}</td>
                  <td class="ad-col-actions" onclick={(e) => e.stopPropagation()}>
                    {#if (session.can('change', 'user') || session.can('delete', 'user')) && u.id !== meId}
                      <div class="mwrap">
                        <button type="button" class="d2-btn ghost icon sm" aria-haspopup="menu" aria-expanded={menuFor === `u${u.id}`} aria-label={t('m.admin.more_actions', { PH: u.username })} onclick={() => (menuFor = menuFor === `u${u.id}` ? null : `u${u.id}`)}><Icon name="three-dots" /></button>
                        {#if menuFor === `u${u.id}`}
                          <Menu label={t('m.admin.more_actions', { PH: u.username })} onclose={() => (menuFor = null)}>
                            {#if session.can('change', 'user')}
                              <button type="button" role="menuitem" class="d2-menu-item" onclick={() => setActive(u, !u.is_active)}>{u.is_active ? t('m.admin.lock') : t('m.admin.unlock')}</button>
                            {/if}
                            {#if session.can('delete', 'user')}
                              <button type="button" role="menuitem" class="d2-menu-item danger" onclick={() => open('users', u.id, true)}><Icon name="trash" />{t('common.delete')}</button>
                            {/if}
                          </Menu>
                        {/if}
                      </div>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      {:else if !shownGroups.length}
        <div class="d2-empty"><p>{t('m.admin.no_groups')}</p></div>
      {:else}
        <table class="ad-table">
          <thead>
            <tr>
              <th scope="col">{t('common.name')}</th>
              <th scope="col">{t('m.admin.col_role')}</th>
              <th scope="col">{t('m.admin.col_members')}</th>
              <th scope="col" class="ad-col-actions"><span class="d2-sr">{t('common.actions')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each shownGroups as g (g.id)}
              {@const r = groupRole(g)}
              <tr class:on={sel?.kind === 'groups' && sel.id === g.id} onclick={() => open('groups', g.id)}>
                <td><button type="button" class="ad-rowbtn" onclick={(e) => { e.stopPropagation(); open('groups', g.id) }}>{g.name}</button></td>
                <td><span class:ad-muted={r.muted}>{r.label}</span></td>
                <td>{memberCount(g.id)}</td>
                <td class="ad-col-actions" onclick={(e) => e.stopPropagation()}>
                  {#if session.can('delete', 'group')}
                    <div class="mwrap">
                      <button type="button" class="d2-btn ghost icon sm" aria-haspopup="menu" aria-expanded={menuFor === `g${g.id}`} aria-label={t('m.admin.more_actions', { PH: g.name })} onclick={() => (menuFor = menuFor === `g${g.id}` ? null : `g${g.id}`)}><Icon name="three-dots" /></button>
                      {#if menuFor === `g${g.id}`}
                        <Menu label={t('m.admin.more_actions', { PH: g.name })} onclose={() => (menuFor = null)}>
                          <button type="button" role="menuitem" class="d2-menu-item danger" onclick={() => open('groups', g.id, true)}><Icon name="trash" />{t('common.delete')}</button>
                        </Menu>
                      {/if}
                    </div>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>

  {#if sel && info && users && groups}
    {#key sel.n}
      {#if sel.kind === 'users'}
        <UserPanel
          bind:this={panelRef}
          user={sel.id === null ? null : (users.find((u) => u.id === sel!.id) ?? null)}
          {groups}
          {info}
          {meId}
          {meSuper}
          canChange={session.can('change', 'user')}
          canDelete={session.can('delete', 'user')}
          startDelete={!!sel.del}
          hint={sel.id === null ? null : (hints.users[String(sel.id)] ?? null)}
          onclose={close}
          onsaved={savedUser}
          ondeleted={deleted}
        />
      {:else}
        <GroupPanel
          bind:this={panelRef}
          group={sel.id === null ? null : (groups.find((g) => g.id === sel!.id) ?? null)}
          {users}
          {info}
          {meSuper}
          canChange={session.can('change', 'group')}
          canDelete={session.can('delete', 'group')}
          canChangeUsers={session.can('change', 'user')}
          startDelete={!!sel.del}
          hint={sel.id === null ? null : (hints.groups[String(sel.id)] ?? null)}
          onclose={close}
          onsaved={savedUser}
          ondeleted={deleted}
        />
      {/if}
    {/key}
  {/if}
</div>

<style>
  .mwrap {
    position: relative;
    display: inline-block;
  }
  :global(.d2-menu-item.danger) {
    color: var(--d2-danger);
  }
</style>
