<!-- AI review list (route automation/review, formerly ai/review): open AI suggestions per document, hooked into the
     "Automation" area (docs/WORKFLOWS-NEU.md). Data: GET /api/x-ai/review/?status=open|… (only
     documents the user may see), document values via v1 /api/documents/?id__in=…. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { api } from '../../api/client'
  import { queryString } from '../../api/v1'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import ReviewCard from './ReviewCard.svelte'
  import { aiStatus } from './status.svelte'
  import { applyReview, listReviews, type Review } from './api'
  import { applyBody, certainty, openIndexes, type DocNow } from './review'

  const PAGE = 20
  type Tab = 'open' | 'done'
  let tab = $state<Tab>('open')
  let reviews = $state<Review[]>([])
  let count = $state(0)
  let page = $state(1)
  let loading = $state(true)
  let failed = $state(false)
  let bulkBusy = $state(false)
  const docs = $state<Record<number, DocNow>>({})

  const status = $derived(aiStatus.data)
  const canEdit = $derived(!!status?.can_review)

  async function loadDocs(ids: number[]) {
    const need = ids.filter((id) => !docs[id])
    if (!need.length) return
    try {
      const r = await api.get<{ results: (DocNow & { id: number })[] }>(
        `/api/documents/${queryString({ id__in: need.join(','), page_size: 100, fields: 'id,title,created,correspondent,document_type,storage_path,tags,custom_fields' })}`,
      )
      for (const d of r.results) docs[d.id] = d
    } catch {
      // without document values the list shows only the suggestions
    }
  }

  async function load(reset = true) {
    loading = true
    failed = false
    if (reset) page = 1
    try {
      const statusParam = tab === 'open' ? 'open' : undefined
      const r = await listReviews({ status: statusParam, page, pageSize: PAGE })
      const rows = tab === 'done' ? r.results.filter((x) => x.status !== 'pending' && x.status !== 'partial') : r.results
      reviews = reset ? rows : [...reviews, ...rows]
      count = r.count
      await loadDocs(rows.map((x) => x.document))
    } catch {
      failed = true
    } finally {
      loading = false
    }
  }

  onMount(() => {
    if (!aiStatus.data) aiStatus.refresh()
    load()
  })

  function changed(r: Review) {
    reviews = reviews.map((x) => (x.id === r.id ? r : x))
    // Reload document values (accepted fields)
    delete docs[r.document]
    loadDocs([r.document])
    aiStatus.refresh()
  }

  function setTab(next: Tab) {
    if (tab === next) return
    tab = next
    load()
  }

  /** Accept all certain suggestions of the loaded documents */
  const sureTotal = $derived(reviews.reduce((n, r) => n + openIndexes(r).filter((i) => certainty(r.changes[i].confidence) === 'sure').length, 0))
  async function applyAllSure() {
    bulkBusy = true
    let ok = 0
    let errors = 0
    for (const r of reviews) {
      const idx = openIndexes(r).filter((i) => certainty(r.changes[i].confidence) === 'sure')
      if (!idx.length) continue
      try {
        changed(await applyReview(r.id, applyBody(idx, {})))
        ok++
      } catch {
        errors++
      }
    }
    bulkBusy = false
    if (errors) toastService.showError(t('m.ai.bulk_partly', { PH: ok, PH_1: errors }))
    else toastService.show({ content: t('m.ai.bulk_done', { PH: ok }), delay: 5000, level: 'success' })
  }
</script>

<div class="rl" id="d2-ai-review">
  <header class="d2-pane-head head">
    <h1><Icon name="stars" />{t('m.ai.review_title')}</h1>
    <div class="seg" role="tablist" aria-label={t('m.ai.review_title')}>
      <button role="tab" aria-selected={tab === 'open'} class:on={tab === 'open'} onclick={() => setTab('open')} id="d2-ai-tab-open">
        {t('m.ai.tab_open')}{#if status?.pending}<span class="cnt">{status.pending}</span>{/if}
      </button>
      <button role="tab" aria-selected={tab === 'done'} class:on={tab === 'done'} onclick={() => setTab('done')} id="d2-ai-tab-done">{t('m.ai.tab_done')}</button>
    </div>
    <span class="grow"></span>
    {#if tab === 'open' && canEdit && sureTotal > 0}
      <button class="d2-btn primary sm" disabled={bulkBusy} onclick={applyAllSure} id="d2-ai-apply-all-sure"><Icon name="check2" />{t('m.ai.apply_all_sure', { PH: sureTotal })}</button>
    {/if}
    <button class="d2-btn ghost sm icon" onclick={() => load()} title={t('m.ai.reload')} aria-label={t('m.ai.reload')}><Icon name="arrow-clockwise" /></button>
  </header>

  <div class="body d2-scroll">
    {#if status && !status.enabled}
      <div class="d2-empty">
        <Icon name="stars" size="2.5rem" />
        <p>{t('m.ai.off_hint')}</p>
        {#if status.can_configure}<a class="d2-btn" href={router.href('settings/ai')}><Icon name="gear" />{t('m.ai.open_settings')}</a>{/if}
      </div>
    {:else if failed}
      <div class="d2-empty">
        <Icon name="exclamation-triangle" size="2rem" />
        <p>{t('m.ai.load_failed')}</p>
        <button class="d2-btn" onclick={() => load()}>{t('m.ai.reload')}</button>
      </div>
    {:else if !loading && reviews.length === 0}
      <div class="d2-empty">
        <Icon name="check-circle" size="2.5rem" />
        <p>{tab === 'open' ? t('m.ai.nothing_open') : t('m.ai.nothing_done')}</p>
        {#if status?.auto_mode === 'off' && tab === 'open'}<p class="d2-muted">{t('m.ai.auto_off_hint')}</p>{/if}
      </div>
    {:else}
      <p class="d2-muted intro">{t('m.ai.review_intro')}</p>
      <div class="list">
        {#each reviews as r (r.id)}
          <ReviewCard review={r} doc={docs[r.document] ?? null} {canEdit} onchange={changed} />
        {/each}
      </div>
      {#if reviews.length < count && tab === 'open'}
        <button class="d2-btn more" disabled={loading} onclick={() => { page += 1; load(false) }}>{t('m.ai.load_more')}</button>
      {/if}
      {#if loading}<p class="d2-muted" aria-busy="true">{t('common.loading')}</p>{/if}
    {/if}
  </div>
</div>

<style>
  .rl {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--d2-bg);
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--d2-s3);
  }
  h1 {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-xl);
  }
  .grow {
    flex: 1;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r1);
    overflow: hidden;
  }
  .seg button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 var(--d2-s3);
    border: 0;
    background: var(--d2-surface);
    color: var(--d2-text-2);
    font: inherit;
    cursor: pointer;
  }
  .seg button.on {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  .cnt {
    font-size: var(--d2-fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .body {
    flex: 1;
    min-height: 0;
    padding: var(--d2-s4);
  }
  .intro {
    max-width: 70ch;
    margin-bottom: var(--d2-s3);
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s3);
    max-width: 1100px;
  }
  .more {
    margin-top: var(--d2-s3);
  }
</style>
