<!-- Port of app.component.html + file-drop (frame). Screens are loaded lazily per route
     (own JS and CSS chunk, FRONTEND-LIGHT §1: every further route ≤ 40 KB lazy). -->
<script lang="ts">
  import { onMount, type Component } from 'svelte'
  import { t, fullLocale } from './lib/i18n'
  import { router } from './router.svelte'
  import AppFrame from './components/app-frame/AppFrame.svelte'
  import ModalHost from './components/common/confirm-dialog/ModalHost.svelte'
  import Toasts from './components/common/toasts/Toasts.svelte'
  import FileDrop from './components/file-drop/FileDrop.svelte'
  import { api } from './api/client'
  import { session, rawValue } from './stores/settings.svelte'
  import { data } from './stores/data.svelte'
  import { toastService } from './stores/toasts.svelte'
  import { consumerStatus } from './stores/uploads.svelte'

  type Loader = () => Promise<{ default: Component<any> }>
  /** Route → [host tag, loader]; first matching rule wins */
  const ROUTES: [RegExp, string, Loader][] = [
    [/^dashboard$/, 'pngx-dashboard', () => import('./components/dashboard/Dashboard.svelte')],
    [/^(documents|view\/\d+)$/, 'pngx-document-list', () => import('./components/document-list/DocumentList.svelte')],
    [/^documents\/\d+(\/(details|content|metadata|notes|history|permissions|preview|duplicates))?$/, 'pngx-document-detail', () => import('./components/document-detail/DocumentDetail.svelte')],
    // S7/S8 administration + settings (second P4 agent)
    [/^asn\/\d+$/, 'pngx-document-asn', () => import('./components/document-asn/DocumentAsn.svelte')],
    [/^attributes(\/[^/]+)?$/, 'pngx-document-attributes', () => import('./components/manage/document-attributes/DocumentAttributes.svelte')],
    // "System" tab (system settings, docs/SETTINGS.md §4.1)
    [/^settings\/system(\/[a-z_]+)?$/, 'pngx-settings-system', () => import('./components/admin/settings/system/System.svelte')],
    [/^settings(\/(general|documents|permissions|notifications))?$/, 'pngx-settings', () => import('./components/admin/settings/Settings.svelte')],
    [/^savedviews$/, 'pngx-saved-views', () => import('./components/manage/saved-views/SavedViews.svelte')],
    [/^workflows$/, 'pngx-workflows', () => import('./components/manage/workflows/Workflows.svelte')],
    [/^mail$/, 'pngx-mail', () => import('./components/manage/mail/Mail.svelte')],
    [/^usersgroups$/, 'pngx-users-groups', () => import('./components/admin/users-groups/UsersGroups.svelte')],
    [/^config$/, 'pngx-config', () => import('./components/admin/config/Config.svelte')],
    [/^tasks$/, 'pngx-tasks', () => import('./components/admin/tasks/Tasks.svelte')],
    [/^logs$/, 'pngx-logs', () => import('./components/admin/logs/Logs.svelte')],
    [/^trash$/, 'pngx-trash', () => import('./components/admin/trash/Trash.svelte')],
    // ASN label generator (PaperRust extension, /rs/api/v2/asn)
    [/^tools\/asn$/,'pngx-asn-labels', () => import('./components/tools/asn-labels/AsnLabels.svelte')],
    [/^share-links$/, 'pngx-share-links', () => import('./components/manage/share-links/ShareLinks.svelte')],
    // Scanner integration (PaperRust extension, /api/x-scan): scan and manage scanners/devices
    [/^tools\/scan$/, 'pngx-scan', () => import('./components/tools/scan/Scan.svelte')],
    [/^tools\/scanners$/, 'pngx-scanners', () => import('./components/tools/scanners/Scanners.svelte')],
    // Ransomware protection (PaperRust extension, /api/x-guard): guard events, read-only mode, backups
    [/^protection$/, 'pngx-protection', () => import('./components/tools/protection/Protection.svelte')],
    // everything else: NotFoundComponent (Angular: '**' → /404)
    [/^.*$/, 'pngx-not-found', () => import('./components/not-found/NotFound.svelte')],
  ]

  const route = $derived(ROUTES.find(([re]) => re.test(router.path)))
  // keep loaded components (no flicker when switching back)
  const loaded = $state<Record<string, Component<any>>>({})
  $effect(() => {
    const r = route
    if (r && !loaded[r[1]]) Promise.all([r[2](), r[1] === 'pngx-document-list' ? null : fullLocale()]).then(([m]) => (loaded[r[1]] = m.default))
  })
  // ComponentRouterService: last visited page (target of "Close" in the document view)
  let current = router.path + router.query
  $effect(() => {
    const next = router.path + router.query
    if (next !== current) {
      ;(window as any).__pngxPreviousPath = current
      current = next
    }
  })

  // AppComponent.ngOnInit: connect websocket, processing status messages
  /** Notification setting (default on; may be stored as a string) */
  const notify = (key: string) => {
    const v = rawValue(session.settings, key)
    return v === undefined ? true : typeof v === 'string' ? v === 'true' : !!v
  }
  /** TasksService.reload (counter "needs attention" in the sidebar) */
  const reloadTasks = () => {
    if (session.can('view', 'paperlesstask')) api.taskStatusCounts().then((r) => (data.needsAttentionTasks = r.needs_attention)).catch(() => {})
  }
  onMount(() => {
    consumerStatus.connect()
    const offs = [
      consumerStatus.onDocumentConsumptionFinished((status) => {
        reloadTasks()
        if (notify('general-settings:notifications:consumer-success')) {
          const canView = session.can('view', 'document')
          // the upload list already shows this, so only record it in the notifications menu
          toastService.show({
            content: t('common.document_added_paperrust', { PH: status.filename }),
            delay: 10000,
            level: 'success',
            silent: true,
            actionName: canView ? t('common.open_document') : undefined,
            action: canView ? () => router.navigate('documents/' + status.documentId) : undefined,
          })
        }
      }),
      consumerStatus.onDocumentConsumptionFailed((status) => {
        reloadTasks()
        if (notify('general-settings:notifications:consumer-failed')) toastService.showError(t('common.could_not_add', { PH: status.filename, PH_1: status.message }))
      }),
      consumerStatus.onDocumentDetected(() => reloadTasks()),
    ]
    return () => {
      offs.forEach((off) => off())
      consumerStatus.disconnect()
    }
  })
</script>

<pngx-toasts><Toasts /></pngx-toasts><pngx-file-drop><FileDrop><pngx-app-frame><AppFrame>
  {#if route}
    {#if loaded[route[1]]}
      {@const C = loaded[route[1]]}
      {#if route[1] === 'pngx-document-list'}
        <pngx-document-list><C /></pngx-document-list>
      {:else if route[1] === 'pngx-document-detail'}
        <pngx-document-detail><C /></pngx-document-detail>
      {:else if route[1] === 'pngx-document-attributes'}
        <pngx-document-attributes><C /></pngx-document-attributes>
      {:else if route[1] === 'pngx-dashboard'}
        <pngx-dashboard><C /></pngx-dashboard>
      {:else if route[1] === 'pngx-settings'}
        <pngx-settings><C /></pngx-settings>
      {:else if route[1] === 'pngx-settings-system'}
        <pngx-settings-system><C /></pngx-settings-system>
      {:else if route[1] === 'pngx-saved-views'}
        <pngx-saved-views><C /></pngx-saved-views>
      {:else if route[1] === 'pngx-workflows'}
        <pngx-workflows><C /></pngx-workflows>
      {:else if route[1] === 'pngx-mail'}
        <pngx-mail><C /></pngx-mail>
      {:else if route[1] === 'pngx-users-groups'}
        <pngx-users-groups><C /></pngx-users-groups>
      {:else if route[1] === 'pngx-config'}
        <pngx-config><C /></pngx-config>
      {:else if route[1] === 'pngx-tasks'}
        <pngx-tasks><C /></pngx-tasks>
      {:else if route[1] === 'pngx-logs'}
        <pngx-logs><C /></pngx-logs>
      {:else if route[1] === 'pngx-trash'}
        <pngx-trash><C /></pngx-trash>
      {:else if route[1] === 'pngx-asn-labels'}
        <pngx-asn-labels><C /></pngx-asn-labels>
      {:else if route[1] === 'pngx-document-asn'}
        <pngx-document-asn><C /></pngx-document-asn>
      {:else if route[1] === 'pngx-share-links'}
        <pngx-share-links><C /></pngx-share-links>
      {:else if route[1] === 'pngx-not-found'}
        <pngx-not-found><C /></pngx-not-found>
      {:else if route[1] === 'pngx-scan'}
        <pngx-scan><C /></pngx-scan>
      {:else if route[1] === 'pngx-scanners'}
        <pngx-scanners><C /></pngx-scanners>
      {:else if route[1] === 'pngx-protection'}
        <pngx-protection><C /></pngx-protection>
      {/if}
    {/if}
  {/if}
</AppFrame></pngx-app-frame></FileDrop></pngx-file-drop><ModalHost />
