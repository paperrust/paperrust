<!-- "Year bar" component: timeline of a year (GET /rs/api/v2/timeline), lanes by document type or tag
     (switch in the component, without setup), month axis, deadlines (diamond), terms (bars), document dates
     (dot). Click → document in the workspace. Tags per document come from the v1 list (id__in, only id/tags). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { queryString } from '../../api/v1'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { timeline, Unavailable, type TimelineData, type TimelineItem } from '../api/assist'
  import { groupItems, isDeadline, months, place, position, yearRange, NO_GROUP, type LaneGrouping } from '../lib/timeline'
  import Widget from './Widget.svelte'
  import Unavail from './TimelineUnavailable.svelte'
  import Icon from '../components/Icon.svelte'
  import { kindLabel } from './kinds'

  const GROUP_KEY = 'paperrust.home.year.group'
  const todayIso = new Date().toISOString().slice(0, 10)
  let year = $state(new Date().getFullYear())
  let showDocs = $state(false)
  let by = $state<LaneGrouping>(readGroup())
  let tl = $state<TimelineData | null>(null)
  let tagsByDoc = $state(new Map<number, number[]>())
  let phase = $state<'loading' | 'ready' | 'unavailable' | 'error'>('loading')
  let error = $state('')

  function readGroup(): LaneGrouping {
    try {
      return localStorage.getItem(GROUP_KEY) === 'tag' ? 'tag' : 'type'
    } catch {
      return 'type'
    }
  }
  function setGroup(g: LaneGrouping) {
    by = g
    try {
      localStorage.setItem(GROUP_KEY, g)
    } catch {}
  }

  const range = $derived(yearRange(year))
  const axis = $derived(months(range.from, range.to))
  const lanes = $derived(
    tl
      ? groupItems(
          tl.items,
          by,
          by === 'type' ? t('m.home.without_document_type') : t('m.home.without_tag'),
          (id) => tagsByDoc.get(id) ?? [],
          (id) => data.tags.get(id),
        )
      : [],
  )
  const placed = $derived(place(lanes, range.from, range.to, 0.028, showDocs ? [] : ['created']))
  const visible = $derived(placed.filter((p) => p.items.some((i) => i.row >= 0 || i.w > 0)))
  const todayX = $derived(todayIso >= range.from && todayIso <= range.to ? position(todayIso, range.from, range.to) : null)
  const monthFmt = new Intl.DateTimeFormat(undefined, { month: 'short' })
  const dateFmt = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
  const fmt = (d: string) => dateFmt.format(new Date(d + 'T00:00:00'))

  let grid: HTMLDivElement | undefined = $state()
  // Shift the timeline so that "today" is visible (narrow windows)
  $effect(() => {
    if (!grid || !tl || todayX === null || grid.scrollWidth <= grid.clientWidth) return
    const lab = grid.querySelector<HTMLElement>('.lane .lab, .axis .lab')?.offsetWidth ?? 0
    const x = todayX
    requestAnimationFrame(() => grid && (grid.scrollLeft = lab + x * (grid.scrollWidth - lab) - grid.clientWidth / 2))
  })

  async function load() {
    phase = 'loading'
    try {
      tl = await timeline(range.from, range.to)
      phase = 'ready'
    } catch (e: any) {
      if (e instanceof Unavailable) phase = 'unavailable'
      else {
        phase = 'error'
        error = e?.message ?? String(e)
      }
    }
  }
  $effect(() => {
    void year
    load()
  })

  /** Reload the documents' tags (only for grouping by tag, one query per 100 documents) */
  async function loadTags(items: TimelineItem[]) {
    const ids = [...new Set(items.map((i) => i.document?.id).filter((x): x is number => x != null))].filter((id) => !tagsByDoc.has(id))
    if (!ids.length) return
    const next = new Map(tagsByDoc)
    for (let i = 0; i < ids.length; i += 100) {
      const chunk = ids.slice(i, i + 100)
      try {
        const r = await api.get<{ results: { id: number; tags?: number[] }[] }>(`/api/documents/${queryString({ page: 1, page_size: chunk.length, id__in: chunk.join(','), fields: 'id,tags', truncate_content: true })}`)
        for (const d of r.results) next.set(d.id, d.tags ?? [])
      } catch {
        // without tags: entries land in "Ohne Tag"
      }
      for (const id of chunk) if (!next.has(id)) next.set(id, [])
    }
    tagsByDoc = next
  }
  $effect(() => {
    if (by === 'tag' && tl) loadTags(tl.items)
  })

  const open = (i: TimelineItem) => i.document && router.navigate('documents/' + i.document.id)
</script>

<Widget id="year" title={t('m.home.year_bar')} icon="calendar3">
  {#snippet tools()}
    <div class="seg" role="radiogroup" aria-label={t('m.home.group_by')}>
      <button role="radio" aria-checked={by === 'type'} class:on={by === 'type'} onclick={() => setGroup('type')} id="d2-year-by-type">{t('common.document_type')}</button>
      <button role="radio" aria-checked={by === 'tag'} class:on={by === 'tag'} onclick={() => setGroup('tag')} id="d2-year-by-tag">{t('m.home.by_tag')}</button>
    </div>
    <label class="d2-check small"><input type="checkbox" bind:checked={showDocs} id="d2-tl-docs" />{t('m.timeline.show_document_dates')}</label>
    <div class="year" role="group" aria-label={t('m.timeline.year')}>
      <button class="d2-btn ghost sm icon" aria-label={t('common.previous_year')} onclick={() => year--}><Icon name="chevron-left" /></button>
      <strong aria-live="polite">{year}</strong>
      <button class="d2-btn ghost sm icon" aria-label={t('m.timeline.next_year')} onclick={() => year++}><Icon name="chevron-right" /></button>
      {#if year !== new Date().getFullYear()}<button class="d2-btn sm" onclick={() => (year = new Date().getFullYear())}>{t('common.today')}</button>{/if}
    </div>
  {/snippet}
  {#if phase === 'unavailable'}
    <Unavail />
  {:else if phase === 'error'}
    <div role="alert" class="err"><p>{error}</p><button class="d2-btn sm" onclick={load}>{t('common.retry')}</button></div>
  {:else}
    {#if tl?.demo}<p class="d2-muted demo">{t('common.demo_data')}</p>{/if}
    <div class="tl" aria-label={t('m.timeline.timeline', { PH: year })} role="region">
      <div class="grid d2-scroll" id="d2-timeline" bind:this={grid}>
        <div class="inner">
          <div class="axis" aria-hidden="true">
            <div class="lab"></div>
            <div class="months">
              {#each axis as m (m.key)}
                <span style:left={`${m.start * 100}%`} style:width={`${m.width * 100}%`}>{monthFmt.format(new Date(m.key + '-01T00:00:00'))}</span>
              {/each}
            </div>
          </div>
          {#if phase === 'loading' && !tl}
            <p class="d2-muted pad">{t('common.loading')}</p>
          {:else if visible.length === 0}
            <p class="d2-muted pad">{t('m.timeline.no_entries_in', { PH: year })}</p>
          {:else}
            {#each visible as p (p.lane.key)}
              <div class="lane" class:none={p.lane.key === NO_GROUP} style:--rows={p.rows} style:--spans={p.spanRows} style:--c={p.lane.color ?? 'var(--d2-text-3)'}>
                <div class="lab"><span class="sw"></span><span class="nm" title={p.lane.name}>{p.lane.name}</span><span class="d2-muted cnt">{p.items.length}</span></div>
                <div class="track">
                  {#each axis as m (m.key)}<span class="mline" style:left={`${m.start * 100}%`}></span>{/each}
                  {#if todayX !== null}<span class="today" style:left={`${todayX * 100}%`}></span>{/if}
                  {#each p.items as it (it.item.id + it.row)}
                    {#if it.w > 0}
                      <span class="span" style:left={`${it.x * 100}%`} style:width={`${it.w * 100}%`} style:top={`${6 + it.spanRow * 8}px`} aria-hidden="true"></span>
                    {/if}
                    {#if it.row >= 0}
                      <button
                        class="mk"
                        class:dl={isDeadline(it.item.kind)}
                        class:doc={it.item.kind === 'created'}
                        class:done={it.item.done}
                        class:past={it.item.date < todayIso}
                        style:left={`${it.at * 100}%`}
                        style:top={`${10 + p.spanRows * 8 + it.row * 24}px`}
                        onclick={() => open(it.item)}
                        aria-label={`${kindLabel(it.item.kind)}: ${it.item.title}, ${fmt(it.item.date)}`}
                        title={`${kindLabel(it.item.kind)} · ${fmt(it.item.date)}\n${it.item.title}${it.item.label ? '\n' + it.item.label : ''}`}
                      ></button>
                    {/if}
                  {/each}
                </div>
              </div>
            {/each}
          {/if}
        </div>
      </div>
      <div class="legend" aria-hidden="true">
        <span><i class="mk dl"></i>{t('m.timeline.deadline')}</span><span><i class="span-l"></i>{t('m.timeline.term_warranty')}</span><span><i class="mk doc"></i>{t('m.timeline.document_date')}</span><span><i class="todayl"></i>{t('common.today')}</span>
      </div>
    </div>
  {/if}
</Widget>

<style>
  .small {
    font-size: var(--d2-fs-sm);
    color: var(--d2-text-2);
  }
  .year {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r1);
    overflow: hidden;
  }
  .seg button {
    padding: 2px var(--d2-s2);
    border: 0;
    background: var(--d2-surface);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
    cursor: pointer;
  }
  .seg button + button {
    border-left: 1px solid var(--d2-border-strong);
  }
  .seg button.on {
    background: var(--d2-accent-soft, var(--d2-surface-2));
    color: var(--d2-accent-text);
    font-weight: 600;
  }
  .seg button:focus-visible {
    outline: 2px solid var(--d2-accent);
    outline-offset: -2px;
  }
  .err {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
  }
  .demo {
    margin: 0 0 var(--d2-s2);
    font-size: var(--d2-fs-xs);
  }
  .tl {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    overflow: hidden;
  }
  .grid {
    max-height: 420px;
  }
  .inner {
    min-width: 760px;
  }
  .axis,
  .lane {
    display: grid;
    grid-template-columns: 170px minmax(0, 1fr);
  }
  .axis {
    position: sticky;
    top: 0;
    z-index: 4;
    height: 30px;
    background: var(--d2-surface);
    border-bottom: 1px solid var(--d2-border);
  }
  .months {
    position: relative;
  }
  .months span {
    position: absolute;
    top: 0;
    bottom: 0;
    display: flex;
    align-items: center;
    padding-left: 6px;
    border-left: 1px solid var(--d2-border);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-xs);
    font-weight: 600;
    text-transform: uppercase;
  }
  .lab {
    position: sticky;
    left: 0;
    z-index: 3;
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    padding: 0 var(--d2-s3);
    background: var(--d2-surface);
    border-right: 1px solid var(--d2-border);
    font-size: var(--d2-fs-sm);
    font-weight: 600;
  }
  .lane.none .lab {
    color: var(--d2-text-2);
    font-weight: 500;
  }
  .sw {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 3px;
    background: var(--c);
  }
  .nm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cnt {
    font-size: var(--d2-fs-xs);
    font-weight: 500;
  }
  .lane {
    min-height: calc(var(--spans) * 8px + var(--rows) * 24px + 16px);
    border-bottom: 1px solid var(--d2-border);
  }
  .track {
    position: relative;
  }
  .mline {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px dashed var(--d2-border);
  }
  .today {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 1;
    border-left: 2px solid var(--d2-info);
  }
  .span {
    position: absolute;
    height: 5px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--c) 45%, transparent);
  }
  .mk {
    position: absolute;
    z-index: 2;
    width: 18px;
    height: 18px;
    margin-left: -9px;
    padding: 0;
    border: 2px solid var(--d2-surface);
    border-radius: 50%;
    background: var(--c);
    cursor: pointer;
  }
  .mk.dl {
    border-radius: 3px;
    transform: rotate(45deg) scale(0.85);
    background: var(--d2-warn);
    box-shadow: 0 0 0 1px var(--d2-warn);
  }
  .mk.dl.past {
    background: var(--d2-text-3);
    box-shadow: 0 0 0 1px var(--d2-text-3);
  }
  .mk.doc {
    width: 12px;
    height: 12px;
    margin-left: -6px;
    margin-top: 3px;
    background: color-mix(in srgb, var(--c) 70%, var(--d2-surface));
  }
  .mk.done {
    opacity: 0.45;
  }
  .mk:hover,
  .mk:focus-visible {
    z-index: 3;
    outline: 2px solid var(--d2-text);
    outline-offset: 2px;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s4);
    padding: 6px var(--d2-s3);
    border-top: 1px solid var(--d2-border);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-xs);
  }
  .legend span {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .legend i {
    position: static;
    display: inline-block;
    margin: 0;
    --c: var(--d2-text-3);
  }
  .legend .mk.dl {
    width: 10px;
    height: 10px;
  }
  .span-l {
    width: 20px;
    height: 6px;
    border-radius: 3px;
    background: var(--d2-text-3);
  }
  .todayl {
    width: 2px;
    height: 12px;
    background: var(--d2-info);
  }
  .pad {
    padding: var(--d2-s5);
  }
  @media (max-width: 767.98px) {
    .axis,
    .lane {
      grid-template-columns: 112px minmax(0, 1fr);
    }
  }
</style>
