<!-- Command palette (Ctrl+K, docs/DESIGN2.md §5): commands + targets + document search (v1 /api/search/); typed text
     shows hits and – only with AI on – the line "Ask question: …" (answer view, docs/DESIGN2-ENTWIRREN.md §5). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { queryString } from '../../api/v1'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { ui } from '../stores/ui.svelte'
  import { askPath, fuzzyScore, rank, textActions, type Entry } from '../lib/palette'
  import { QUICK_FILTERS, queryFromState, setText } from '../lib/filters'
  import Icon from './Icon.svelte'
  import { flashSetting, searchSettings, staticEntries, systemEntries, type SettingEntry } from '../lib/settings-index'

  interface Item extends Entry {
    icon: string
    hint?: string
    run: () => void
  }

  let q = $state(ui.paletteQuery)
  let sel = $state(0)
  let input: HTMLInputElement
  let docs = $state<Item[]>([])
  let objs = $state<Item[]>([])
  let searching = $state(false)
  const today = new Date().toISOString().slice(0, 10)
  const close = () => (ui.paletteOpen = false)
  const nav = (p: string) => () => {
    router.navigate(p)
    close()
  }

  const aiOnNav = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const commands = $derived<Item[]>([
    { id: 'go-overview', group: t('m.ui.go'), label: t('common.dashboard'), keywords: 'start overview übersicht timeline zeitstrahl dashboard', icon: 'grid-1x2', hint: 'g o', run: nav('dashboard') },
    { id: 'go-inbox', group: t('m.ui.go'), label: t('common.inbox'), icon: 'inbox', hint: 'g i', run: nav('inbox') },
    { id: 'go-docs', group: t('m.ui.go'), label: t('common.all_documents'), icon: 'files', hint: 'g d', run: nav('documents') },
    { id: 'go-assistant', group: t('m.ui.go'), label: t('common.assistant'), keywords: 'chat ai ki ask', icon: 'stars', hint: 'g a', run: nav('assistant') },
    { id: 'go-tags', group: t('m.ui.go'), label: t('common.tags'), icon: 'tags', run: nav('attributes/tags') },
    { id: 'go-corr', group: t('m.ui.go'), label: t('common.correspondents'), icon: 'person', run: nav('attributes/correspondents') },
    { id: 'go-types', group: t('m.ui.go'), label: t('common.document_types'), icon: 'file-earmark-text', run: nav('attributes/documenttypes') },
    { id: 'go-paths', group: t('m.ui.go'), label: t('common.storage_paths'), icon: 'folder', run: nav('attributes/storagepaths') },
    { id: 'go-cf', group: t('m.ui.go'), label: t('common.custom_fields'), icon: 'card-text', run: nav('attributes/customfields') },
    { id: 'go-views', group: t('m.ui.go'), label: t('common.saved_views'), icon: 'bookmark', run: nav('savedviews') },
    // "Automation" area (docs/WORKFLOWS-NEU.md): workflows, email fetching, AI review list, new workflow
    { id: 'go-wf', group: t('m.ui.go'), label: t('common.workflows'), keywords: 'automatik automation workflow regeln', icon: 'diagram-3', run: nav('automation/workflows') },
    { id: 'go-mailflows', group: t('m.ui.go'), label: t('m.auto.mail_title'), keywords: 'automatik automation mail e-mail regel abruf imap', icon: 'envelope', run: nav('automation/mail') },
    ...(aiOnNav && session.can('view', 'document') ? [{ id: 'go-review', group: t('m.ui.go'), label: t('m.ai.review_title'), keywords: 'ki ai vorschläge prüfen review', icon: 'stars', run: nav('automation/review') }] : []),
    ...(session.can('add', 'workflow') ? [{ id: 'new-wf', group: t('common.actions'), label: t('m.auto.new_workflow'), keywords: 'automatik automation workflow anlegen erstellen vorlage', icon: 'plus-lg', run: nav('automation/workflows?new=1') }] : []),
    { id: 'go-mail', group: t('m.ui.go'), label: t('common.mail'), icon: 'envelope', run: nav('mail') },
    { id: 'go-trash', group: t('m.ui.go'), label: t('common.trash'), icon: 'trash', run: nav('trash') },
    { id: 'go-settings', group: t('m.ui.go'), label: t('common.settings'), icon: 'gear', hint: 'g s', run: nav('settings') },
    ...data.sidebarViews.map((v) => ({ id: 'view-' + v.id, group: t('common.views'), label: v.name, icon: 'bookmark', run: nav('view/' + v.id) })),
    ...QUICK_FILTERS.map((f) => ({ id: 'q-' + f.id, group: t('m.ui.quick_filters'), label: t(f.label), icon: f.icon, run: nav('documents' + queryFromState({ rules: f.rules(today), sortField: 'created', sortReverse: true })) })),
    { id: 'upload', group: t('common.actions'), label: t('common.upload_documents'), keywords: 'hochladen', icon: 'cloud-arrow-up', hint: 'u', run: () => (close(), document.getElementById('d2-upload-input')?.click()) },
    { id: 'theme', group: t('common.actions'), label: t('common.switch_light_dark'), keywords: 'theme dark hell dunkel', icon: 'circle-half', hint: 't', run: () => (close(), ui.cycleTheme()) },
    { id: 'help', group: t('common.actions'), label: t('common.keyboard_shortcuts'), icon: 'keyboard', hint: '?', run: () => (close(), (ui.helpOpen = true)) },
    { id: 'classic', group: t('common.actions'), label: t('m.ui.switch_classic_interface'), keywords: 'design klassisch', icon: 'layout-three-columns', run: () => import('../../lib/design-apply').then((m) => m.applyDesign('classic')) },
  ])

  // Settings (global search, at the bottom of the list): click opens the page and highlights the setting
  let sysEntries = $state<SettingEntry[]>([])
  void systemEntries().then((e) => (sysEntries = e))
  const settingEntries = $derived([...staticEntries(), ...sysEntries])
  const openSetting = (e: SettingEntry) => () => {
    close()
    router.navigate(e.route)
    void flashSetting(e)
  }
  const settingItems = $derived<Item[]>(
    q.trim().length < 2
      ? []
      : searchSettings(settingEntries, q, 8).map((e) => ({ id: e.id, group: t('common.settings'), label: e.label, keywords: e.keywords, hint: e.where, icon: 'gear', run: openSetting(e) })),
  )
  const aiOn = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const actions = $derived(textActions(q, aiOn))
  const fulltext = $derived<Item[]>(
    actions
      .filter((a) => a.kind === 'fulltext')
      .map((a) => ({ id: 'fulltext', group: t('common.documents_2'), label: t('m.ui.search_documents', { PH: a.query }), icon: 'search', run: nav('documents' + queryFromState({ rules: setText([], a.query), sortField: 'created', sortReverse: true })) })),
  )
  const askLine = $derived<Item[]>(
    actions.filter((a) => a.kind === 'ask').map((a) => ({ id: 'ask', group: t('common.assistant'), label: t('m.ui.ask_question', { PH: a.query }), icon: 'stars', run: nav(askPath(a.query)) })),
  )
  // strong hits on commands/targets (text contained) first, then full text + document hits, own line
  // "Ask question", objects, rest
  const ranked = $derived(rank(commands, q, q.trim() ? 8 : 30))
  const strong = $derived(q.trim() ? ranked.filter((c) => fuzzyScore(q, c.label) >= 50) : [])
  const results = $derived<Item[]>([...strong, ...fulltext, ...docs, ...askLine, ...objs, ...ranked.filter((c) => !strong.includes(c)), ...settingItems])
  $effect(() => {
    if (sel >= results.length) sel = Math.max(0, results.length - 1)
  })

  let timer: ReturnType<typeof setTimeout> | null = null
  let seq = 0
  $effect(() => {
    const query = q.trim()
    if (timer) clearTimeout(timer)
    if (query.length < 2) {
      docs = []
      objs = []
      return
    }
    timer = setTimeout(async () => {
      const my = ++seq
      searching = true
      try {
        const r = await api.get<any>(`/api/search/${queryString({ query })}`)
        if (my !== seq) return
        docs = (r.documents ?? []).slice(0, 6).map((d: any) => ({ id: 'doc-' + d.id, group: t('common.documents_2'), label: d.title, icon: 'file-earmark', hint: d.created?.slice(0, 10), run: nav('documents/' + d.id) }))
        const o = (list: any[] | undefined, icon: string, path: (x: any) => string) => (list ?? []).slice(0, 3).map((x: any) => ({ id: icon + x.id, group: t('common.filter'), label: x.name, icon, run: nav(path(x)) }))
        objs = [
          ...o(r.tags, 'tag', (x) => 'documents?tags__id__all=' + x.id),
          ...o(r.correspondents, 'person', (x) => 'documents?correspondent__id__in=' + x.id),
          ...o(r.document_types, 'file-earmark-text', (x) => 'documents?document_type__id__in=' + x.id),
        ]
        // Workflows and email rules from the global search open the "Automation" area
        const wf = (list: any[] | undefined, icon: string, path: (x: any) => string, group: string) => (list ?? []).slice(0, 3).map((x: any) => ({ id: 'auto-' + icon + x.id, group, label: x.name, icon, run: nav(path(x)) }))
        objs = [
          ...objs,
          ...wf(r.workflows, 'diagram-3', (x) => 'automation/workflows/' + x.id, t('common.workflows')),
          ...wf(r.mail_rules, 'envelope', (x) => 'automation/mail/' + x.id, t('m.auto.mail_title')),
        ]
      } catch {
        docs = []
        objs = []
      } finally {
        if (my === seq) searching = false
      }
    }, 180)
  })

  onMount(() => input.focus())
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close()
    } else if (e.key === 'ArrowDown') {
      sel = (sel + 1) % Math.max(1, results.length)
      e.preventDefault()
    } else if (e.key === 'ArrowUp') {
      sel = (sel - 1 + results.length) % Math.max(1, results.length)
      e.preventDefault()
    } else if (e.key === 'Enter') {
      results[sel]?.run()
      e.preventDefault()
    }
  }
  $effect(() => {
    document.getElementById(`d2-pal-${sel}`)?.scrollIntoView({ block: 'nearest' })
  })
</script>

<div class="backdrop" role="presentation" onclick={close}></div>
<div class="pal" role="dialog" aria-modal="true" aria-label={t('m.ui.command_palette')} id="d2-palette">
  <div class="inp">
    <Icon name="search" size="1.1rem" />
    <input
      bind:this={input}
      bind:value={q}
      {onkeydown}
      placeholder={aiOn ? t('m.ui.search_documents_commands_ask') : t('m.ui.search_documents_commands')}
      role="combobox"
      aria-expanded="true"
      aria-controls="d2-pal-list"
      aria-activedescendant={results.length ? `d2-pal-${sel}` : undefined}
      aria-label={t('m.ui.search_run_command')}
      autocomplete="off"
      spellcheck="false"
    />
    {#if searching}<span class="d2-muted small">{t('m.ui.searching')}</span>{/if}
    <span class="d2-kbd">Esc</span>
  </div>
  <ul id="d2-pal-list" role="listbox" class="d2-scroll" aria-label={t('common.results')}>
    {#each results as r, i (r.id)}
      {#if i === 0 || results[i - 1].group !== r.group}<li class="grp" role="presentation">{r.group}</li>{/if}
      <li id="d2-pal-{i}" role="option" aria-selected={i === sel} class:sel={i === sel} onpointermove={() => (sel = i)} onclick={() => r.run()} onkeydown={() => {}}>
        <Icon name={r.icon} />
        <span class="lbl">{r.label}</span>
        {#if r.hint}<span class="hint">{r.hint}</span>{/if}
      </li>
    {:else}
      <li class="none" role="presentation">{t('m.ui.no_results')}</li>
    {/each}
  </ul>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 95;
    background: var(--d2-overlay);
  }
  .pal {
    position: fixed;
    top: 12vh;
    left: 50%;
    z-index: 96;
    width: min(640px, calc(100vw - 24px));
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    transform: translateX(-50%);
    border: 1px solid var(--d2-border);
    border-radius: 14px;
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    overflow: hidden;
  }
  .inp {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    padding: 0 var(--d2-s4);
    border-bottom: 1px solid var(--d2-border);
    color: var(--d2-text-3);
  }
  input {
    flex: 1;
    height: 52px;
    border: 0;
    background: none;
    color: var(--d2-text);
    font-size: var(--d2-fs-lg);
    outline: none;
  }
  .small {
    font-size: var(--d2-fs-xs);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: var(--d2-s2);
  }
  .grp {
    padding: var(--d2-s2) var(--d2-s2) 2px;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    font-weight: 600;
  }
  [role='option'] {
    display: flex;
    align-items: center;
    gap: var(--d2-s3);
    padding: 8px var(--d2-s2);
    border-radius: var(--d2-r1);
    cursor: pointer;
  }
  [role='option'].sel {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
  }
  .lbl {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
  }
  .none {
    padding: var(--d2-s4);
    color: var(--d2-text-3);
    text-align: center;
  }
</style>
