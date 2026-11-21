<!-- Fixed navigation (docs/DESIGN2-ENTWIRREN.md §6): home, inbox, all documents, AI assistant (only with AI on),
     automation, saved views ("in sidebar"), at the bottom administration and settings. No quick filters.
     ≥ 1100 px with labels (the user can collapse it to the icon bar, stored per browser), 768–1099 px as icon bar,
     < 768 px as drawer. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import { ui } from '../stores/ui.svelte'
  import { counts } from '../stores/counts.svelte'
  import { aiStatus } from '../ai/status.svelte'
  import { stateFromQuery } from '../lib/filters'
  import Icon from './Icon.svelte'
  import { SYMBOL } from '../lib/icons'

  const rail = $derived(ui.rail)
  const isActive = (p: string) => router.path === p || router.path.startsWith(p + '/')
  const inList = $derived(router.path === 'documents' || /^documents\/\d+/.test(router.path))
  const listRules = $derived(inList ? stateFromQuery(router.query).rules : null)
  const manageActive = $derived(/^(attributes|savedviews|workflows|mail|usersgroups|roles|config|settings\/system|tasks|logs|trash|tools|protection|share-links)(\/|$)/.test(router.path))
  // same permissions as the areas in manage/sections.ts (administrators always see something)
  const MANAGE_TYPES = ['tag', 'correspondent', 'documenttype', 'storagepath', 'customfield', 'savedview', 'document', 'workflow', 'mailaccount', 'paperlesstask']
  const canAutomation = $derived(session.can('view', 'workflow') || (aiStatus.enabled && session.can('view', 'document')))
  const aiOn = $derived(aiStatus.enabled || !!session.get(SETTINGS_KEYS.AI_ENABLED))
  const canManage = $derived(session.isAdmin || MANAGE_TYPES.some((p) => session.can('view', p)))

  onMount(() => {
    counts.refresh()
    aiStatus.refresh()
    return consumerStatus.onDocumentConsumptionFinished(() => {
      counts.refresh()
      aiStatus.refresh()
    })
  })
  $effect(() => {
    void router.path
    if (ui.mobile) ui.navOpen = false
  })
</script>

{#snippet item(href: string, icon: string, label: string, active: boolean, count: number | null, id: string)}
  <a class="it" class:active {href} aria-current={active ? 'page' : undefined} title={rail ? label : undefined} aria-label={rail ? label : undefined} id={id || undefined}>
    <Icon name={icon} size="1.05rem" />
    {#if !rail}<span class="lbl">{label}</span>{/if}
    {#if count !== null && !rail}<span class="cnt">{count}</span>{/if}
  </a>
{/snippet}

{#if ui.mobile && ui.navOpen}
  <div class="scrim" role="presentation" onclick={() => (ui.navOpen = false)}></div>
{/if}
<nav class="nav" class:rail class:drawer={ui.mobile} class:open={ui.navOpen} aria-label={t('m.ui.main_navigation')}>
  <div class="d2-scroll inner">
    <ul>
      <li>{@render item(router.href('dashboard'), 'grid-1x2', t('common.dashboard'), router.path === 'dashboard', null, 'd2-nav-overview')}</li>
      <li>{@render item(router.href('inbox'), 'inbox', t('common.inbox'), router.path === 'inbox', counts.inbox, 'd2-nav-inbox')}</li>
      <li>{@render item(router.href('documents'), 'files', t('common.all_documents'), inList && (!listRules || listRules.length === 0), counts.all, 'd2-nav-documents')}</li>
      {#if aiOn}<li>{@render item(router.href('assistant'), 'stars', t('m.ui.ai_assistant'), router.path === 'assistant', null, 'd2-nav-assistant')}</li>{/if}
      <!-- "Automation" area (workflows, email fetching, AI review list – docs/WORKFLOWS-NEU.md topic 10 B); counter = open AI suggestions -->
      {#if canAutomation}<li>{@render item(router.href('automation/workflows'), SYMBOL.workflow, t('m.auto.title'), isActive('automation'), aiStatus.enabled ? aiStatus.pending : null, 'd2-nav-automation')}</li>{/if}
    </ul>

    {#if data.sidebarViews.length}
      {#if !rail}<h2 class="sec">{t('common.views')}</h2>{:else}<hr />{/if}
      <ul id="d2-nav-views">
        {#each data.sidebarViews as v (v.id)}
          <li>{@render item(router.href('view/' + v.id), 'bookmark', v.name, router.path === 'view/' + v.id, data.viewCounts[v.id] ?? null, '')}</li>
        {/each}
      </ul>
    {/if}
  </div>
  <ul class="bottom">
    {#if canManage}
      <li>{@render item(router.href('attributes/tags'), SYMBOL.management, t('common.management'), manageActive, null, 'd2-nav-manage')}</li>
    {/if}
    <li>{@render item(router.href('settings'), 'gear', t('common.settings'), isActive('settings'), null, 'd2-nav-settings')}</li>
    {#if !ui.mobile && !ui.compact}
      <li>
        <button class="it toggle" type="button" id="d2-nav-toggle" aria-expanded={!ui.navCollapsed} aria-label={ui.navCollapsed ? t('m.ui.expand_sidebar') : t('m.ui.collapse_sidebar')} title={ui.navCollapsed ? t('m.ui.expand_sidebar') : t('m.ui.collapse_sidebar')} onclick={() => ui.toggleNav()}>
          <Icon name={ui.navCollapsed ? 'chevron-right' : 'chevron-left'} size="1.05rem" />
          {#if !rail}<span class="lbl">{t('m.ui.collapse_sidebar')}</span>{/if}
        </button>
      </li>
    {/if}
  </ul>
</nav>

<style>
  .nav {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--d2-bg);
    border-right: 1px solid var(--d2-border);
  }
  .inner {
    flex: 1;
    padding: var(--d2-s3) var(--d2-s2);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .bottom {
    padding: var(--d2-s2);
    border-top: 1px solid var(--d2-border);
  }
  .sec {
    margin: var(--d2-s4) var(--d2-s2) var(--d2-s1);
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  hr {
    margin: var(--d2-s2);
    border: 0;
    border-top: 1px solid var(--d2-border);
  }
  .it {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    height: var(--d2-item-h);
    padding: 0 var(--d2-s2) 0 var(--d2-s3);
    border-radius: var(--d2-r1);
    color: var(--d2-text-2);
    font-weight: 500;
    text-decoration: none;
  }
  .toggle {
    width: 100%;
    border: 0;
    background: none;
    cursor: pointer;
    font: inherit;
    color: var(--d2-text-3);
    text-align: left;
  }
  .it:hover {
    background: var(--d2-surface-2);
    color: var(--d2-text);
  }
  .it.active {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  .lbl {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cnt {
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .rail .it {
    justify-content: center;
    padding: 0;
    height: 40px;
  }
  .drawer {
    position: fixed;
    top: var(--d2-header-h);
    bottom: 0;
    left: 0;
    z-index: 50;
    width: min(300px, 86vw);
    transform: translateX(-100%);
    transition: transform 0.18s ease;
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    visibility: hidden;
  }
  .drawer.open {
    transform: none;
    visibility: visible;
  }
  .scrim {
    position: fixed;
    inset: var(--d2-header-h) 0 0 0;
    z-index: 49;
    background: var(--d2-overlay);
  }
</style>
