<!-- Administration in the PaperRust design (docs/DESIGN2-ENTWIRREN.md §6): on the left two areas – "Organization" and "System", each
     entry by permission (users/roles/logs/configuration administrators only, sections.ts) –, on the right the existing component of the classic Light UI
     (same routes, same logic and dialogs) or an own component of the PaperRust design (design2/admin/*).
     Dialogs appear as a side sheet (classic.css). Visibility: sections.ts. -->
<script lang="ts">
  import 'bootstrap/dist/css/bootstrap.css'
  import '../../styles/global.scss'
  import './classic.css'
  import type { Component } from 'svelte'
  import { t, msg } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import ModalHost from '../../components/common/confirm-dialog/ModalHost.svelte'
  import Icon from '../components/Icon.svelte'
  import { ui } from '../stores/ui.svelte'
  import { allowedSections, fetchRolesMode, rolesModeOf, sectionFor, SECTION_ICON as I, visibleSections, type Area, type RolesMode, type SectionDef } from './sections'

  type Mod = { default: Component<any> }
  type Loader = () => Promise<Mod>

  // Own components of the PaperRust design (design2/admin/)
  const usersAdmin: Loader = () => import('../admin/UsersAdmin.svelte')
  const rolesAdmin: Loader = () => import('../admin/RolesAdmin.svelte')

  interface View {
    /** Key in the cache and for the host tag */
    host: string
    load: Loader
    /** own component of the PaperRust design (without classic host/Bootstrap frame) */
    native?: boolean
  }
  interface Sec extends SectionDef {
    view: (path: string) => View | null
  }
  const attrs: View = { host: 'document-attributes', load: () => import('../../components/manage/document-attributes/DocumentAttributes.svelte') }
  const v = (host: string, load: Loader, native = false) => () => ({ host, load, native })
  const SECTIONS: Sec[] = [
    // Organization
    { path: 'attributes/tags', also: ['attributes'], label: msg('common.tags'), icon: I.tags, area: 'order', perm: 'tag', view: () => attrs },
    { path: 'attributes/correspondents', label: msg('common.correspondents'), icon: I.correspondents, area: 'order', perm: 'correspondent', view: () => attrs },
    { path: 'attributes/documenttypes', label: msg('common.document_types'), icon: I.documentTypes, area: 'order', perm: 'documenttype', view: () => attrs },
    { path: 'attributes/storagepaths', label: msg('common.storage_paths'), icon: I.storagePaths, area: 'order', perm: 'storagepath', view: () => attrs },
    { path: 'attributes/customfields', label: msg('m.workspace.fields'), icon: I.customFields, area: 'order', perm: 'customfield', view: () => attrs },
    { path: 'savedviews', label: msg('common.saved_views'), icon: I.savedViews, area: 'order', perm: 'savedview', view: v('saved-views', () => import('../../components/manage/saved-views/SavedViews.svelte')) },
    { path: 'share-links', label: msg('common.share_links'), icon: I.shareLinks, area: 'order', perm: 'sharelink', view: v('share-links', () => import('../../components/manage/share-links/ShareLinks.svelte')) },
    { path: 'tools/asn', label: msg('common.asn_labels'), icon: I.asn, area: 'order', perm: 'document', view: v('asn-labels', () => import('../../components/tools/asn-labels/AsnLabels.svelte')) },
    // System (administrators only)
    {
      path: 'usersgroups',
      label: msg('common.users_groups'),
      icon: I.usersGroups,
      area: 'system',
      perm: 'user',
      adminOnly: true,
      view: v('users-admin', usersAdmin, true),
    },
    // Navigation entry only in "custom" mode; the route stays reachable for administrators in both modes
    // (the classic/custom switch is located there, link from the user management).
    { path: 'roles', label: msg('m.manage.roles'), icon: I.roles, area: 'system', perm: 'user', adminOnly: true, customRolesOnly: true, view: v('roles-admin', rolesAdmin, true) },
    { path: 'tools/scan', label: msg('scan.title'), icon: I.scan, area: 'system', perm: 'document', permAction: 'add', view: v('scan', () => import('../../components/tools/scan/Scan.svelte')) },
    { path: 'tools/scanners', label: msg('scanners.title'), icon: I.scanners, area: 'system', perm: null, superuserOnly: true, view: v('scanners', () => import('../../components/tools/scanners/Scanners.svelte')) },
    { path: 'protection', label: msg('protection.title'), icon: I.protection, area: 'system', perm: null, superuserOnly: true, view: v('protection', () => import('../../components/tools/protection/Protection.svelte')) },
    { path: 'workflows', label: msg('common.workflows'), icon: I.workflows, area: 'system', perm: 'workflow', view: v('workflows', () => import('../../components/manage/workflows/Workflows.svelte')) },
    { path: 'mail', label: msg('common.mail'), icon: I.mail, area: 'system', perm: 'mailaccount', view: v('mail', () => import('../../components/manage/mail/Mail.svelte')) },
    { path: 'tasks', label: msg('m.manage.file_tasks'), icon: I.tasks, area: 'system', perm: 'paperlesstask', view: v('tasks', () => import('../../components/admin/tasks/Tasks.svelte')) },
    { path: 'logs', label: msg('common.logs'), icon: I.logs, area: 'system', perm: null, adminOnly: true, view: v('logs', () => import('../../components/admin/logs/Logs.svelte')) },
    { path: 'trash', label: msg('common.trash'), icon: I.trash, area: 'system', perm: 'document', permAction: 'delete', view: v('trash', () => import('../../components/admin/trash/Trash.svelte')) },
    {
      // Configuration = Application (classic config) + Server (formerly Settings → System, route settings/system[/group])
      path: 'config',
      also: ['settings/system'],
      label: msg('common.configuration'),
      icon: I.config,
      area: 'system',
      perm: 'applicationconfiguration',
      adminOnly: true,
      view: (p) =>
        /^settings\/system(\/|$)/.test(p)
          ? { host: 'settings-system', load: () => import('../../components/admin/settings/system/System.svelte') }
          : { host: 'config', load: () => import('../../components/admin/config/Config.svelte') },
    },
  ]
  const AREAS: { k: Area; label: string }[] = [
    { k: 'order', label: msg('m.manage.order') },
    { k: 'system', label: msg('common.system') },
  ]

  let rolesMode = $state<RolesMode>('classic')
  let rolesKnown = $state(false)
  fetchRolesMode().then((m) => ((rolesMode = m), (rolesKnown = true)))
  // Switching on the roles page (api/roles.ts) → adjust navigation immediately
  $effect(() => {
    const on = (e: Event) => {
      rolesMode = rolesModeOf(200, { mode: (e as CustomEvent).detail })
      rolesKnown = true
    }
    window.addEventListener('paperrust:roles-mode', on)
    return () => window.removeEventListener('paperrust:roles-mode', on)
  })

  const admin = $derived(!!(session.data?.user?.is_superuser || session.data?.user?.is_staff))
  const viewer = $derived({ can: (p: string, a: 'view' | 'add' | 'delete') => session.can(a, p), admin, superuser: !!session.data?.user?.is_superuser, rolesMode })
  /** in the navigation */
  const visible = $derived(visibleSections(SECTIONS, viewer) as Sec[])
  /** allowed as a route (e.g. roles also in classic mode) */
  const allowed = $derived(allowedSections(SECTIONS, viewer) as Sec[])
  const current = $derived(sectionFor(allowed, router.path) as Sec | undefined)
  const view = $derived(current?.view(router.path) ?? null)
  /** Route belongs to the administration, but is not allowed → first allowed area */
  const denied = $derived(!current && rolesKnown && !!session.data && sectionFor(SECTIONS, router.path) !== undefined)
  $effect(() => {
    if (denied && visible.length) router.navigate(visible[0].path, true)
  })

  const cache = $state<Record<string, Component<any>>>({})
  $effect(() => {
    const w = view
    if (w && !cache[w.host]) w.load().then((m) => (cache[w.host] = m.default))
  })
  const areas = $derived(AREAS.filter((a) => visible.some((s) => s.area === a.k)))
  const configTabs = [
    { path: 'config', label: msg('m.manage.config_app') },
    { path: 'settings/system', label: msg('m.manage.config_server') },
  ]
  const configActive = $derived(/^settings\/system(\/|$)/.test(router.path) ? 'settings/system' : 'config')
</script>

<div class="mh" class:mobile={ui.mobile}>
  <nav class="sub d2-scroll" aria-label={t('common.management')}>
    {#if ui.mobile}
      <label class="d2-sr" for="d2-manage-select">{t('common.management')}</label>
      <select id="d2-manage-select" class="d2-select" value={current?.path} onchange={(e) => router.navigate(e.currentTarget.value)}>
        {#each areas as a (a.k)}
          <optgroup label={t(a.label)}>
            {#each visible.filter((s) => s.area === a.k) as s (s.path)}<option value={s.path}>{t(s.label)}</option>{/each}
          </optgroup>
        {/each}
      </select>
    {:else}
      {#each areas as a (a.k)}
        <h2 id="d2-manage-area-{a.k}">{t(a.label)}</h2>
        <ul aria-labelledby="d2-manage-area-{a.k}">
          {#each visible.filter((s) => s.area === a.k) as s (s.path)}
            <li><a href={router.href(s.path)} class:on={current?.path === s.path} aria-current={current?.path === s.path ? 'page' : undefined} id="d2-manage-{s.path.replace(/\//g, '-')}"><Icon name={s.icon} />{t(s.label)}</a></li>
          {/each}
        </ul>
      {/each}
    {/if}
  </nav>
  <div class="d2-scroll pane" class:d2-classic={!view?.native} id="d2-classic-pane">
    {#if current?.path === 'config'}
      <div class="subtabs" role="tablist" aria-label={t('common.configuration')}>
        {#each configTabs as c (c.path)}
          <a role="tab" href={router.href(c.path)} aria-selected={configActive === c.path} class:on={configActive === c.path}>{t(c.label)}</a>
        {/each}
      </div>
    {/if}
    {#if !visible.length && session.data}
      <p class="d2-muted">{t('m.manage.no_sections')}</p>
    {:else if current && !view}
      <p class="d2-muted" id="d2-manage-unavailable">{t('m.manage.not_available_yet')}</p>
    {:else if view && cache[view.host]}
      {@const C = cache[view.host]}
      {#if view.native}<C />
      {:else if view.host === 'document-attributes'}<pngx-document-attributes sh-document-attributes=""><C /></pngx-document-attributes>
      {:else if view.host === 'saved-views'}<pngx-saved-views sh-saved-views=""><C /></pngx-saved-views>
      {:else if view.host === 'workflows'}<pngx-workflows sh-workflows=""><C /></pngx-workflows>
      {:else if view.host === 'mail'}<pngx-mail sh-mail=""><C /></pngx-mail>
      {:else if view.host === 'asn-labels'}<pngx-asn-labels sh-asn-labels=""><C /></pngx-asn-labels>
      {:else if view.host === 'users-groups'}<pngx-users-groups sh-users-groups=""><C /></pngx-users-groups>
      {:else if view.host === 'tasks'}<pngx-tasks sh-tasks=""><C /></pngx-tasks>
      {:else if view.host === 'logs'}<pngx-logs sh-logs=""><C /></pngx-logs>
      {:else if view.host === 'trash'}<pngx-trash sh-trash=""><C /></pngx-trash>
      {:else if view.host === 'config'}<pngx-config sh-config=""><C /></pngx-config>
      {:else if view.host === 'settings-system'}<pngx-settings-system sh-settings-system=""><C /></pngx-settings-system>
      {:else if view.host === 'share-links'}<pngx-share-links sh-share-links=""><C /></pngx-share-links>
      {:else if view.host === 'scan'}<pngx-scan sh-scan=""><C /></pngx-scan>
      {:else if view.host === 'scanners'}<pngx-scanners sh-scanners=""><C /></pngx-scanners>
      {:else if view.host === 'protection'}<pngx-protection sh-protection=""><C /></pngx-protection>
      {/if}
    {:else}
      <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
    {/if}
  </div>
</div>
<ModalHost />

<style>
  .mh {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  .mh.mobile {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(0, 1fr);
  }
  .sub {
    padding: var(--d2-s3) var(--d2-s2);
    border-right: 1px solid var(--d2-border);
    background: var(--d2-bg);
  }
  .mobile .sub {
    border-right: 0;
    border-bottom: 1px solid var(--d2-border);
  }
  .pane:not(.d2-classic) {
    background: var(--d2-surface);
  }
  h2 {
    margin: var(--d2-s3) var(--d2-s2) var(--d2-s1) !important;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .sub a {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    height: var(--d2-item-h);
    padding: 0 var(--d2-s2);
    border-radius: var(--d2-r1);
    color: var(--d2-text-2);
    font-weight: 500;
    text-decoration: none;
  }
  .sub a:hover {
    background: var(--d2-surface-2);
  }
  .sub a.on {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  /* Sub-tabs of the configuration (Application · Server) */
  .subtabs {
    display: flex;
    gap: 2px;
    margin: calc(-1 * var(--d2-s4)) calc(-1 * var(--d2-s5)) var(--d2-s4);
    padding: 0 var(--d2-s5);
    border-bottom: 1px solid var(--d2-border);
  }
  .subtabs a {
    position: relative;
    display: flex;
    align-items: center;
    height: 40px;
    padding: 0 var(--d2-s3);
    color: var(--d2-text-2);
    font-weight: 500;
    text-decoration: none;
  }
  .subtabs a.on {
    color: var(--d2-text);
    font-weight: 600;
  }
  .subtabs a.on::after {
    content: '';
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: -1px;
    height: 2px;
    background: var(--d2-accent);
  }
  /* Server settings: the classic component's own tab bar duplicates the sub-tabs */
  .pane :global(pngx-settings-system > .nav-tabs) {
    display: none;
  }
</style>
