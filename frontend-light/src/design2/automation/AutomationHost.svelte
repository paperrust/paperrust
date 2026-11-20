<!-- "Automation" area (topic 10 B, docs/WORKFLOWS-NEU.md): workflows, email fetching, AI review list (only with AI).
     Routes: automation[/workflows], automation/workflows/new|:id, automation/mail, automation/mail/new|:id,
     automation/review. Each part appears only with the matching permission. -->
<script lang="ts">
  import './automation.css'
  import { onMount, type Component } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { ui } from '../stores/ui.svelte'
  import { aiStatus } from '../ai/status.svelte'
  import Icon from '../components/Icon.svelte'
  import { SYMBOL } from '../lib/icons'
  import { auto } from './store.svelte'
  import { registerAutomationIcons } from './icons'

  registerAutomationIcons()

  type Part = 'workflows' | 'mail' | 'review'
  const canWorkflows = $derived(session.can('view', 'workflow'))
  const canMail = $derived(session.can('view', 'workflow') && session.can('view', 'mailrule'))
  const canReview = $derived(aiStatus.enabled && session.can('view', 'document'))
  const parts = $derived(
    [
      canWorkflows ? { k: 'workflows' as Part, path: 'automation/workflows', label: t('common.workflows'), icon: SYMBOL.workflow, count: null as number | null } : null,
      canMail ? { k: 'mail' as Part, path: 'automation/mail', label: t('m.auto.mail_title'), icon: SYMBOL.mail, count: null } : null,
      canReview ? { k: 'review' as Part, path: 'automation/review', label: t('m.ai.review_title'), icon: SYMBOL.ai, count: aiStatus.pending } : null,
    ].filter((x) => x !== null),
  )

  /** Route → part + subpage */
  const route = $derived.by(() => {
    const p = router.path.split('/')
    const k = (p[1] ?? 'workflows') as Part
    return { part: (['workflows', 'mail', 'review'].includes(k) ? k : 'workflows') as Part, id: p[2] ?? null }
  })
  const allowed = $derived(parts.some((x) => x.k === route.part))
  /** AI status queried (before that the review list is not decided yet) */
  let aiChecked = $state(!!aiStatus.data)
  $effect(() => {
    // part not allowed → first allowed one (e.g. review list with AI off)
    if (!allowed && parts.length && (route.part !== 'review' || aiChecked)) router.navigate(parts[0].path, true)
  })

  type Loader = () => Promise<{ default: Component<any> }>
  const VIEWS: Record<string, Loader> = {
    list: () => import('./WorkflowList.svelte'),
    editor: () => import('./WorkflowEditor.svelte'),
    mail: () => import('./MailFlows.svelte'),
    mailEditor: () => import('./MailFlowEditor.svelte'),
    review: () => import('../ai/ReviewList.svelte'),
  }
  const viewKey = $derived(route.part === 'review' ? 'review' : route.part === 'mail' ? (route.id ? 'mailEditor' : 'mail') : route.id ? 'editor' : 'list')
  const cache = $state<Record<string, Component<any>>>({})
  $effect(() => {
    const k = viewKey
    if (!cache[k]) VIEWS[k]().then((m) => (cache[k] = m.default))
  })

  onMount(() => {
    void auto.load()
    if (!aiStatus.data) void aiStatus.refresh().finally(() => (aiChecked = true))
  })
</script>

<div class="au-host" class:mobile={ui.mobile}>
  <nav class="au-sub d2-scroll" aria-label={t('m.auto.title')}>
    {#if ui.mobile}
      <div class="au-subtabs" role="tablist" aria-label={t('m.auto.title')}>
        {#each parts as p (p.k)}
          <a role="tab" href={router.href(p.path)} aria-selected={route.part === p.k} class:on={route.part === p.k}>{p.label}</a>
        {/each}
      </div>
    {:else}
      <h2 id="d2-auto-head">{t('m.auto.title')}</h2>
      <ul aria-labelledby="d2-auto-head">
        {#each parts as p (p.k)}
          <li>
            <a href={router.href(p.path)} class:on={route.part === p.k} aria-current={route.part === p.k ? 'page' : undefined} id="d2-auto-{p.k}">
              <Icon name={p.icon} /><span class="lbl">{p.label}</span>{#if p.count}<span class="cnt">{p.count}</span>{/if}
            </a>
          </li>
        {/each}
      </ul>
    {/if}
  </nav>
  <div class="au-pane">
    {#if !parts.length && session.data}
      <p class="d2-muted au-pad">{t('m.auto.no_access')}</p>
    {:else if cache[viewKey] && allowed}
      {@const C = cache[viewKey]}
      {#key viewKey === 'editor' || viewKey === 'mailEditor' ? `${viewKey}-${route.id}` : viewKey}
        <C id={route.id} />
      {/key}
    {:else}
      <p class="d2-muted au-pad" aria-busy="true">{t('common.loading')}</p>
    {/if}
  </div>
</div>
