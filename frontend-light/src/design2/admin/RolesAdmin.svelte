<!-- Manage → System → Roles (docs/DESIGN2-ENTWIRREN.md §1): role mode classic/custom and – in custom
     mode – create, copy, edit, delete own roles. Administrators (superusers) only. -->
<script lang="ts">
  import './admin.css'
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'
  import { onCommand } from '../lib/bus'
  import { loadRoles, RolesError, setRoleMode } from '../api/roles'
  import { activeRoles, rolePerms, sortRoles, summarize, type Role, type RoleMode, type RolesInfo } from './roles'
  import { roleDesc, roleName, summarySentence } from './labels'
  import RolePanel from './RolePanel.svelte'

  type Sel = { id: number | string | null; copy?: Role | null; del?: boolean; n: number }
  const meSuper = !!(session.data?.user as any)?.is_superuser
  let info = $state<RolesInfo | null>(null)
  let failed = $state(false)
  let sel = $state<Sel | null>(null)
  let menuFor = $state<string | null>(null)
  let modeBusy = $state(false)
  let panelRef = $state<{ requestClose: (then?: () => void) => void } | null>(null)
  let seq = 0

  async function load() {
    failed = false
    try {
      info = await loadRoles()
    } catch {
      failed = true
    }
  }
  onMount(() => {
    if (meSuper) load()
    return onCommand((cmd) => {
      if (cmd === 'escape' && sel && panelRef) {
        panelRef.requestClose()
        return true
      }
      return false
    })
  })

  const list = $derived(info ? sortRoles(activeRoles(info)) : [])
  const custom = $derived(info?.mode === 'custom' && !info.fallback)

  function open(id: number | string | null, extra: Partial<Sel> = {}) {
    menuFor = null
    const go = () => (sel = { id, ...extra, n: ++seq })
    if (sel && panelRef) panelRef.requestClose(go)
    else go()
  }
  async function changeMode(m: RoleMode) {
    if (!info || info.mode === m) return
    modeBusy = true
    try {
      await setRoleMode(m)
      toastService.showSuccess(t('m.admin.mode_saved'))
      sel = null
      await load()
    } catch (e) {
      toastService.showError(e instanceof RolesError ? e.message : t('m.admin.error_save'))
    } finally {
      modeBusy = false
    }
  }
  function done(msg: string) {
    toastService.showSuccess(msg)
    sel = null
    load()
  }
  const selRole = $derived(sel && info && sel.id !== null ? (info.roles.find((r) => String(r.id) === String(sel!.id)) ?? null) : null)
</script>

<div class="ad-page" class:with-panel={!!sel}>
  <div class="ad-main">
    <div class="ad-head">
      <h1>{t('m.admin.roles')}</h1>
      <a class="d2-btn ghost" href={router.href('usersgroups')} onclick={(e) => { e.preventDefault(); router.navigate('usersgroups') }}><Icon name="people" />{t('common.users_groups')}</a>
      {#if custom}<button type="button" class="d2-btn primary" onclick={() => open(null)}><Icon name="plus-lg" />{t('m.admin.add_role')}</button>{/if}
    </div>

    <div class="d2-scroll ad-list">
      {#if !meSuper}
        <div class="d2-empty"><p>{t('m.admin.admins_only')}</p></div>
      {:else if failed}
        <div class="d2-empty"><p>{t('m.admin.error_load')}</p><button type="button" class="d2-btn" onclick={load}>{t('common.retry')}</button></div>
      {:else if !info}
        <div class="d2-empty" aria-busy="true">{t('common.loading')}</div>
      {:else}
        {#if info.fallback}<p class="ad-note">{t('m.admin.api_missing')}</p>{/if}
        <section class="mode" aria-labelledby="ad-mode-h">
          <h2 id="ad-mode-h">{t('m.admin.role_mode')}</h2>
          <div class="cards" role="radiogroup" aria-labelledby="ad-mode-h">
            {#each [['classic', 'm.admin.mode_classic', 'm.admin.mode_classic_hint'], ['custom', 'm.admin.mode_custom', 'm.admin.mode_custom_hint']] as [m, l, h] (m)}
              <label class="card" class:on={info.mode === m}>
                <input type="radio" name="ad-mode" value={m} checked={info.mode === m} disabled={modeBusy || info.fallback} onchange={() => changeMode(m as RoleMode)} />
                <span><span class="n">{t(l)}</span><span class="d">{t(h)}</span></span>
              </label>
            {/each}
          </div>
          <p class="ad-hint">{t('m.admin.mode_change_hint')}</p>
        </section>

        <table class="ad-table">
          <thead>
            <tr>
              <th scope="col">{t('common.name')}</th>
              <th scope="col" class="ad-wide">{t('m.admin.rights')}</th>
              <th scope="col" class="ad-col-actions"><span class="d2-sr">{t('common.actions')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each list as r (r.id)}
              <tr class:on={sel?.id !== null && String(sel?.id) === String(r.id)} onclick={() => open(r.id)}>
                <td>
                  <button type="button" class="ad-rowbtn" onclick={(e) => { e.stopPropagation(); open(r.id) }}>{roleName(r)}</button>
                  {#if r.builtin}<span class="d2-badge">{t('m.admin.builtin')}</span>{/if}
                  <span class="ad-sub">{roleDesc(r)}</span>
                </td>
                <td class="ad-wide ad-muted">{summarySentence(summarize(rolePerms(r), info.topics, r.admin))}</td>
                <td class="ad-col-actions" onclick={(e) => e.stopPropagation()}>
                  {#if custom && !r.admin}
                    <div class="mwrap">
                      <button type="button" class="d2-btn ghost icon sm" aria-haspopup="menu" aria-expanded={menuFor === String(r.id)} aria-label={t('m.admin.more_actions', { PH: roleName(r) })} onclick={() => (menuFor = menuFor === String(r.id) ? null : String(r.id))}><Icon name="three-dots" /></button>
                      {#if menuFor === String(r.id)}
                        <Menu label={t('m.admin.more_actions', { PH: roleName(r) })} onclose={() => (menuFor = null)}>
                          <button type="button" role="menuitem" class="d2-menu-item" onclick={() => open(null, { copy: r })}>{t('m.admin.copy_role')}</button>
                          {#if !r.builtin}<button type="button" role="menuitem" class="d2-menu-item danger" onclick={() => open(r.id, { del: true })}><Icon name="trash" />{t('common.delete')}</button>{/if}
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

  {#if sel && info}
    {#key sel.n}
      <RolePanel
        bind:this={panelRef}
        role={selRole}
        copyOf={sel.copy ?? null}
        {info}
        startDelete={!!sel.del}
        onclose={() => (sel = null)}
        onsaved={done}
        ondeleted={done}
        oncopy={(r) => open(null, { copy: r })}
      />
    {/key}
  {/if}
</div>

<style>
  .mode {
    padding: var(--d2-s4);
    border-bottom: 1px solid var(--d2-border);
  }
  .mode h2 {
    margin: 0 0 var(--d2-s2);
    font-size: var(--d2-fs-lg);
    font-weight: 600;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: var(--d2-s2);
    max-width: 760px;
  }
  .card {
    display: flex;
    align-items: flex-start;
    gap: var(--d2-s2);
    margin: 0;
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
    cursor: pointer;
  }
  .card.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
  }
  .card input {
    margin: 3px 0 0;
    width: 16px;
    height: 16px;
    flex: none;
    accent-color: var(--d2-accent);
  }
  .card .n {
    display: block;
    font-weight: 600;
  }
  .card .d {
    display: block;
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
  }
  .mwrap {
    position: relative;
    display: inline-block;
  }
  :global(.d2-menu-item.danger) {
    color: var(--d2-danger);
  }
  .d2-badge {
    margin-left: var(--d2-s2);
    vertical-align: middle;
  }
</style>
