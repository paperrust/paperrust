<!-- Filter bar of the workspace (docs/DESIGN2-ENTWIRREN.md §5–6): search (title & content, with AI on with a line
     "Ask question: …"), filters as chips (tags, correspondent, document type, storage path, fields, date, permissions),
     sorting. Chips wrap (no horizontal scrolling); on the phone behind the "Filter" button. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { data } from '../../stores/data.svelte'
  import { DOCUMENT_SORT_FIELDS } from '../../stores/listView.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { router } from '../../router.svelte'
  import { ui } from '../stores/ui.svelte'
  import { ws } from '../stores/workspace.svelte'
  import { OBJ_RULES, clearIdSelection, clearObjects, getDate, getIdSelection, getObjects, getText, noneRule, setDate, setText, type ObjKind } from '../lib/filters'
  import { OWNER_MODES, activeFilterCount, getFieldFilter, getOwnerMode, remainingRules, setFields, setOwnerMode, type OwnerMode } from '../lib/filterbar'
  import { askPath } from '../lib/palette'
  import ObjectPicker from '../components/ObjectPicker.svelte'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'

  let text = $state(getText(ws.state.rules))
  let lastRules = ws.state.rules
  $effect(() => {
    // Query changed (back, saved view) → update the input field
    if (ws.state.rules !== lastRules) {
      lastRules = ws.state.rules
      if (document.activeElement?.id !== 'd2-filter-text') text = getText(ws.state.rules)
    }
  })
  let timer: ReturnType<typeof setTimeout> | null = null
  function onText() {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => ws.apply({ rules: setText(ws.state.rules, text) }), 350)
  }

  const opts = (m: Map<number, { id: number; name: string; color?: string }>) => [...m.values()].map((o) => ({ id: o.id, name: o.name, color: o.color })).sort((a, b) => a.name.localeCompare(b.name))
  const KINDS: { kind: ObjKind; label: string; icon: string; src: () => Map<number, any>; none: boolean }[] = [
    { kind: 'tags', label: t('common.tags'), icon: 'tag', src: () => data.tags, none: true },
    { kind: 'correspondent', label: t('common.correspondent'), icon: 'person', src: () => data.correspondents, none: true },
    { kind: 'document_type', label: t('common.document_type'), icon: 'file-earmark-text', src: () => data.documentTypes, none: true },
    { kind: 'storage_path', label: t('common.storage_path'), icon: 'folder', src: () => data.storagePaths, none: true },
  ]
  function setObjects(kind: ObjKind, next: (number | null)[]) {
    let rules = clearObjects(ws.state.rules, kind)
    for (const id of next) {
      if (id === null) {
        const nr = noneRule(kind)
        if (nr) rules = [...rules, nr]
      }
      else rules = [...rules, { rule_type: OBJ_RULES[kind].any, value: String(id) }]
    }
    ws.apply({ rules })
  }

  let dateOpen = $state(false)
  let sortOpen = $state(false)
  const from = $derived(getDate(ws.state.rules, 'from'))
  const to = $derived(getDate(ws.state.rules, 'to'))
  const others = $derived(remainingRules(ws.state.rules))
  // temporary selection of single documents ("open sources" in the assistant)
  const selection = $derived(getIdSelection(ws.state.rules))
  const active = $derived(ws.state.rules.length > 0)
  const sortName = $derived(t(DOCUMENT_SORT_FIELDS.find((f) => f.field === ws.state.sortField)?.name ?? ws.state.sortField))
  const me = $derived<number | null>(session.data?.user?.id ?? null)

  // Fields (custom field present) and permissions (owner/shared), rules like the classic UI
  const fieldOpts = $derived(data.customFields.map((f) => ({ id: f.id, name: f.name })).sort((a, b) => a.name.localeCompare(b.name)))
  const fieldFilter = $derived(getFieldFilter(ws.state.rules))
  let rightsOpen = $state(false)
  const ownerMode = $derived(getOwnerMode(ws.state.rules, me))
  const OWNER_LABEL: Record<OwnerMode, string> = {
    all: 'common.all',
    mine: 'filter.documents',
    shared_with_me: 'filter.shared_3',
    shared_by_me: 'filter.shared_2',
    unowned: 'ui.unowned',
    custom: 'm.workspace.rights_custom',
  }
  const modes = $derived(OWNER_MODES.filter((m) => me != null || m === 'all' || m === 'unowned'))

  // Phone: filters behind a button (counter = set filter groups)
  let filtersOpen = $state(false)
  const filterCount = $derived(activeFilterCount(ws.state.rules, me))

  // "Ask question: …" below the search field (only with AI on), opens the answer view
  const aiOn = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  let textFocus = $state(false)
  let askLink = $state<HTMLAnchorElement>()
  const showAsk = $derived(aiOn && textFocus && text.trim().length > 0)
  function onTextKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      if (timer) clearTimeout(timer)
      ws.apply({ rules: setText(ws.state.rules, text) })
    } else if (e.key === 'ArrowDown' && showAsk) {
      e.preventDefault()
      askLink?.focus()
    } else if (e.key === 'Escape' && showAsk) {
      textFocus = false
    }
  }
  function onSearchFocusOut(e: FocusEvent) {
    const box = e.currentTarget as HTMLElement
    if (!box.contains(e.relatedTarget as Node | null)) textFocus = false
  }
</script>

<div class="bar">
  <div class="top">
    <div class="search" onfocusin={() => (textFocus = true)} onfocusout={onSearchFocusOut}>
      <Icon name="search" />
      <input id="d2-filter-text" type="search" class="d2-input" placeholder={t('m.workspace.search_title_content')} aria-label={t('m.workspace.search_title_content')} bind:value={text} oninput={onText} onkeydown={onTextKey} autocomplete="off" />
      {#if showAsk}
        <a class="ask" id="d2-filter-ask" bind:this={askLink} href={router.href(askPath(text))} onkeydown={(e) => { if (e.key === 'ArrowUp') { e.preventDefault(); document.getElementById('d2-filter-text')?.focus() } }}>
          <Icon name="stars" /><span>{t('m.ui.ask_question', { PH: text.trim() })}</span>
        </a>
      {/if}
    </div>
    {#if ui.mobile}
      <button class="d2-btn ghost sm" id="d2-filter-toggle" aria-expanded={filtersOpen} aria-controls="d2-filter-chips" onclick={() => (filtersOpen = !filtersOpen)}>
        <Icon name="funnel" /><span>{t('common.filter')}</span>{#if filterCount}<span class="d2-badge">{filterCount}</span>{/if}
      </button>
    {/if}
    <div class="pop sort">
      <button class="d2-btn ghost sm" aria-expanded={sortOpen} onclick={() => (sortOpen = !sortOpen)} aria-label={t('m.workspace.sort', { PH: sortName })} id="d2-sort">
        <Icon name={ws.state.sortReverse ? 'sort-down' : 'sort-up'} /><span class="sn">{sortName}</span>
      </button>
      {#if sortOpen}
        <Menu onclose={() => (sortOpen = false)} label={t('common.sort')} width="220px">
          {#each DOCUMENT_SORT_FIELDS as f (f.field)}
            <button class="d2-menu-item" aria-pressed={ws.state.sortField === f.field} onclick={() => { ws.apply({ sortField: f.field }); sortOpen = false }}>
              <span class="ck">{#if ws.state.sortField === f.field}<Icon name="check2" />{/if}</span>{t(f.name)}
            </button>
          {/each}
          <div class="d2-menu-sep"></div>
          <button class="d2-menu-item" onclick={() => ws.apply({ sortReverse: !ws.state.sortReverse })}><Icon name={ws.state.sortReverse ? 'sort-up' : 'sort-down'} />{ws.state.sortReverse ? t('m.workspace.ascending') : t('m.workspace.descending')}</button>
        </Menu>
      {/if}
    </div>
  </div>
  {#if !ui.mobile || filtersOpen}
    <div class="chips" id="d2-filter-chips" role="group" aria-label={t('common.filter')}>
      {#if selection.length}
        <button class="chipbtn on" id="d2-filter-selection" title={t('m.workspace.selection_remove')} aria-label={`${t('m.workspace.selection_from_assistant', { PH: selection.length })} – ${t('m.workspace.selection_remove')}`} onclick={() => ws.apply({ rules: clearIdSelection(ws.state.rules) })}>
          <Icon name="stars" /><span>{t('m.workspace.selection_from_assistant', { PH: selection.length })}</span><Icon name="x-lg" />
        </button>
      {/if}
      {#each KINDS as k (k.kind)}
        <ObjectPicker variant="chip" label={k.label} icon={k.icon} multi allowNone={k.none} options={opts(k.src())} selected={getObjects(ws.state.rules, k.kind)} onchange={(next) => setObjects(k.kind, next)} id={'d2-filter-' + k.kind} />
      {/each}
      {#if fieldFilter.kind === 'query'}
        <button class="chipbtn on" id="d2-filter-fields" title={t('m.workspace.fields_query_hint')} onclick={() => ws.apply({ rules: setFields(ws.state.rules, []) })}>
          <Icon name="card-text" /><span>{t('m.workspace.fields_query')}</span><Icon name="x-lg" />
        </button>
      {:else if fieldOpts.length || fieldFilter.kind === 'exists'}
        <ObjectPicker variant="chip" label={t('m.workspace.fields')} icon="card-text" multi options={fieldOpts} selected={fieldFilter.kind === 'exists' ? fieldFilter.ids : []} onchange={(next) => ws.apply({ rules: setFields(ws.state.rules, next.filter((x): x is number => x !== null)) })} id="d2-filter-fields" />
      {/if}
      <div class="pop">
        <button class="chipbtn" class:on={!!(from || to)} aria-expanded={dateOpen} onclick={() => (dateOpen = !dateOpen)} id="d2-filter-date">
          <Icon name="calendar3" /><span>{from || to ? `${from || '…'} – ${to || '…'}` : t('common.date')}</span><Icon name="chevron-down" />
        </button>
        {#if dateOpen}
          <Menu onclose={() => (dateOpen = false)} label={t('common.created')} align="left" width="260px">
            <div class="d2-menu-label">{t('common.created')}</div>
            <label class="dt"><span>{t('common.from')}</span><input class="d2-input" type="date" value={from} onchange={(e) => ws.apply({ rules: setDate(ws.state.rules, 'from', e.currentTarget.value) })} /></label>
            <label class="dt"><span>{t('common.to')}</span><input class="d2-input" type="date" value={to} onchange={(e) => ws.apply({ rules: setDate(ws.state.rules, 'to', e.currentTarget.value) })} /></label>
          </Menu>
        {/if}
      </div>
      <div class="pop">
        <button class="chipbtn" class:on={ownerMode !== 'all'} aria-expanded={rightsOpen} onclick={() => (rightsOpen = !rightsOpen)} id="d2-filter-rights">
          <Icon name="eye" /><span>{ownerMode === 'all' ? t('m.workspace.rights') : `${t('m.workspace.rights')}: ${t(OWNER_LABEL[ownerMode])}`}</span><Icon name="chevron-down" />
        </button>
        {#if rightsOpen}
          <Menu onclose={() => (rightsOpen = false)} label={t('m.workspace.rights')} align="left" width="240px">
            {#each modes as m (m)}
              <button class="d2-menu-item" aria-pressed={ownerMode === m} onclick={() => { ws.apply({ rules: setOwnerMode(ws.state.rules, m, me) }); rightsOpen = false }}>
                <span class="ck">{#if ownerMode === m}<Icon name="check2" />{/if}</span>{t(OWNER_LABEL[m])}
              </button>
            {/each}
          </Menu>
        {/if}
      </div>
      {#if others.length}
        <span class="d2-chip" title={t('m.workspace.further_rules_view_quick')}>{t('m.workspace.rules', { PH: others.length })}</span>
      {/if}
      {#if active}
        <button class="d2-btn ghost sm" id="d2-filter-reset" onclick={() => ws.apply({ rules: [] })}><Icon name="x-circle" />{t('common.reset')}</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .bar {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    padding: var(--d2-s3) var(--d2-s3) var(--d2-s2);
    border-bottom: 1px solid var(--d2-border);
  }
  .top {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
  }
  .search {
    position: relative;
    flex: 1 1 auto;
    min-width: 0;
    color: var(--d2-text-3);
  }
  .search > :global(.d2-ico) {
    position: absolute;
    left: 10px;
    top: 50%;
    transform: translateY(-50%);
  }
  .search input {
    padding-left: 32px;
  }
  .ask {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    padding: 8px var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-2);
    color: var(--d2-text);
    font-size: var(--d2-fs-sm);
    text-decoration: none;
  }
  .ask:hover,
  .ask:focus-visible {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
  }
  .ask span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* wrap instead of scrolling horizontally (docs/DESIGN2-ENTWIRREN.md §6) */
  .chips {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .chips > :global(*) {
    flex: none;
    max-width: 100%;
  }
  .pop {
    position: relative;
  }
  .sort {
    flex: none;
  }
  .sn {
    max-width: 110px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chipbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 260px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r-pill);
    background: var(--d2-surface);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .chipbtn span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chipbtn.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
  }
  .dt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 4px var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
  .ck {
    display: inline-flex;
    width: 1em;
  }
</style>
