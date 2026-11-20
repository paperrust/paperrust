<!-- Port of app-frame/global-search/global-search.component.html/.ts: input with debouncing (400 ms, from 3
     characters), GET /api/search/ (db_only according to setting), result menu (GlobalSearchResults.svelte, loaded on the
     first search), keyboard control, full-text search via list.quickFilter, shortcut "/".
     Host <pngx-global-search> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/global-search.css'
  import type { Component } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import { rawValue, session } from '../../../stores/settings.svelte'
  import { Dropdown } from '../../../lib/dropdown.svelte'

  // SETTINGS_KEYS.SEARCH_DB_ONLY / SEARCH_FULL_TYPE (data/ui-settings.ts)
  const SEARCH_DB_ONLY = 'general-settings:search:db-only'
  const SEARCH_FULL_TYPE = 'general-settings:search:more-link'
  const DEBOUNCE_MS = 400

  let query = $state<string | null>(null)
  let searchResults = $state<any>(null)
  let loading = $state(false)
  let searchInput: HTMLInputElement
  let Results = $state<Component<any> | null>(null)
  let resultsRef: any = $state()

  const dd = new Dropdown({ onOpenChange: (open) => !open && reset() })

  const useAdvancedForFullSearch = $derived((rawValue(session.settings, SEARCH_FULL_TYPE) ?? 'title-content') === 'advanced')
  function searchDbOnly(): boolean {
    const v = rawValue(session.settings, SEARCH_DB_ONLY)
    if (typeof v === 'string') {
      try {
        return !!JSON.parse(v)
      } catch {
        return false
      }
    }
    return !!v
  }

  // queryDebounce: debounceTime(400), filter(empty or > 2 characters), distinctUntilChanged
  let timer: ReturnType<typeof setTimeout> | undefined
  let lastDebounced: string | null | undefined = undefined
  let seq = 0
  function queryDebounceNext(text: string | null) {
    clearTimeout(timer)
    timer = setTimeout(() => {
      if (text?.length && text.length <= 2) return
      if (text === lastDebounced) return
      lastDebounced = text
      query = text
      if (text) search(text)
    }, DEBOUNCE_MS)
  }

  function onQueryChange(text: string) {
    // set immediately so Enter / the search button do not wait for the debounce
    query = text
    queryDebounceNext(text)
  }

  async function search(q: string) {
    const my = ++seq
    loading = true
    try {
      const [r, M] = await Promise.all([api.globalSearch(q.trim(), searchDbOnly()), Results ? null : import('./GlobalSearchResults.svelte')])
      if (my !== seq) return
      if (M) Results = M.default
      searchResults = r
      loading = false
      dd.openMenu()
      searchInput?.focus() // openMenu focuses the anchor (= input field)
    } catch {
      if (my === seq) loading = false
    }
  }

  function reset(close = false) {
    seq++ // discard running search
    loading = false
    clearTimeout(timer)
    lastDebounced = null
    query = null
    searchResults = null
    if (close) dd.close()
  }

  function searchInputKeyDown(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' && searchResults?.total && dd.open) {
      event.preventDefault()
      resultsRef?.focusFirst()
    } else if (event.key === 'ArrowUp' && searchResults?.total && dd.open) {
      event.preventDefault()
      resultsRef?.focusLast()
    } else if (event.key === 'Enter') {
      event.preventDefault()
      if (searchResults?.total === 1 && dd.open) {
        resultsRef?.clickFirstPrimary()
        searchInput.blur()
      } else if (query?.length) {
        runFullSearch()
        reset(true)
      }
    } else if (event.key === 'Escape' && !dd.open) {
      if (query?.length) reset(true)
      else searchInput.blur()
    }
  }

  /** runFullSearch: default document list with full-text (advanced) or title-&-content rule */
  async function runFullSearch() {
    const text = query
    reset(true)
    if (!text) return
    const [{ list }, gs] = await Promise.all([import('../../../stores/listView.svelte'), import('./global-search')])
    list.quickFilter([{ rule_type: gs.fullSearchRuleType(rawValue(session.settings, SEARCH_FULL_TYPE)), value: text }])
  }

  // HotKeyService: "/" focuses the search (not in input fields)
  $effect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey) return
      const el = e.target as HTMLElement | null
      if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return
      if (document.body.classList.contains('modal-open')) return
      e.preventDefault()
      searchInput?.focus()
    }
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  })
</script>

<div class="dropdown" class:show={dd.open} use:dd.hostEl>
    <form class="form-inline position-relative" onsubmit={(e) => e.preventDefault()}>
        <pngx-bs-icon width="1em" height="1em" name="search">{@html icon('search', '1em', '1em')}</pngx-bs-icon>
        <div class="input-group">
            <div class="form-control form-control-sm">
                <input class="bg-transparent border-0 w-100 h-100" bind:this={searchInput} type="text" name="query"
                    placeholder={t('common.search')} aria-label={t('common.search')}
                    autocomplete="off"
                    spellcheck="false"
                    value={query ?? ''}
                    oninput={(e) => onQueryChange(e.currentTarget.value)}
                    onkeydown={searchInputKeyDown}
                    use:dd.anchorEl>
                <div class="position-absolute top-50 end-0 translate-middle">
                    {#if loading}
                        <div class="spinner-border spinner-border-sm text-muted mt-1"></div>
                    {/if}
                </div>
            </div>
            {#if query}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={runFullSearch}>
                    {#if useAdvancedForFullSearch}
                        {t('common.advanced_search')}
                    {:else}
                        {t('common.search')}
                    {/if}
                    <pngx-bs-icon width="1em" height="1em" name="arrow-right-short">{@html icon('arrow-right-short', '1em', '1em')}</pngx-bs-icon>
                </button>
            {/if}
        </div>
    </form>

    {#if dd.open && Results && searchResults}
        <Results bind:this={resultsRef} results={searchResults} menuEl={dd.menuEl} onreset={() => reset(true)} onfocusinput={() => searchInput?.focus()} />
    {/if}
</div>
