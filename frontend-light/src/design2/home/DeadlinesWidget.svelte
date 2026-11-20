<!-- "Deadlines and appointments" component (formerly "Upcoming"): open deadlines of the next 60 days + overdue ones
     (GET /rs/api/v2/timeline/upcoming), mark done / snooze 7 days (PUT …/timeline/items/{id}/state). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { setItemState, timeline, upcoming, Unavailable, type TimelineItem } from '../api/assist'
  import { daysBetween, upcomingFrom, urgency, yearRange } from '../lib/timeline'
  import Widget from './Widget.svelte'
  import Unavail from './TimelineUnavailable.svelte'
  import Icon from '../components/Icon.svelte'
  import { kindLabel } from './kinds'

  const todayIso = new Date().toISOString().slice(0, 10)
  let soon = $state<TimelineItem[] | null>(null)
  let phase = $state<'loading' | 'ready' | 'unavailable' | 'error'>('loading')
  let error = $state('')
  const dateFmt = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
  const fmt = (d: string) => dateFmt.format(new Date(d + 'T00:00:00'))
  const daysLeft = (i: TimelineItem) => i.daysLeft ?? daysBetween(todayIso, i.date)

  async function load() {
    phase = 'loading'
    try {
      soon = await upcoming(60, 30).catch(async (e) => {
        if (e instanceof Unavailable) throw e
        // Fallback: deadlines from the timeline of the current year
        const r = yearRange(new Date().getFullYear())
        const tl = await timeline(r.from, r.to)
        return upcomingFrom(tl.items, tl.today)
      })
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
    load()
  })

  function left(i: TimelineItem): string {
    const d = daysLeft(i)
    if (d < 0) return t('m.timeline.days_overdue', { PH: -d })
    if (d === 0) return t('common.today')
    if (d === 1) return t('m.timeline.tomorrow')
    return t('m.timeline.in_days', { PH: d })
  }
  async function mark(i: TimelineItem, s: { done?: boolean; snooze_days?: number }) {
    try {
      await setItemState(i.id, s)
      soon = soon?.filter((x) => x.id !== i.id) ?? null
      toastService.showSuccess(s.done ? t('m.timeline.marked_done') : t('m.timeline.snoozed_days', { PH: s.snooze_days ?? 0 }))
    } catch (e) {
      toastService.showError(t('common.could_not_save'), e)
    }
  }
  const open = (i: TimelineItem) => i.document && router.navigate('documents/' + i.document.id)
</script>

<Widget id="deadlines" title={t('m.home.deadlines')} icon="calendar-event">
  {#snippet tools()}<span class="d2-muted sub">{t('m.timeline.next_60_days')}</span>{/snippet}
  {#if phase === 'unavailable'}
    <Unavail />
  {:else if phase === 'error'}
    <div role="alert" class="err"><p>{error}</p><button class="d2-btn sm" onclick={load}>{t('common.retry')}</button></div>
  {:else}
    <ul class="cards" id="d2-upcoming">
      {#if soon === null}
        <li class="d2-muted">{t('common.loading')}</li>
      {:else}
        {#each soon as i (i.id)}
          {@const d = daysLeft(i)}
          <li class="card {urgency(d)}" class:over={d < 0}>
            <div class="when"><span class="left">{left(i)}</span><span class="d2-muted">{fmt(i.date)}</span></div>
            <button class="ttl" onclick={() => open(i)} title={i.title}>{i.title}</button>
            <div class="meta"><span>{kindLabel(i.kind)}</span>{#if i.amount}<span>{i.amount}</span>{/if}</div>
            <div class="acts">
              <button class="d2-btn sm" onclick={() => mark(i, { done: true })} aria-label={t('m.timeline.mark_done', { PH: i.title })}><Icon name="check2" />{t('common.done')}</button>
              <button class="d2-btn ghost sm" onclick={() => mark(i, { snooze_days: 7 })} aria-label={t('m.timeline.snooze_7_days', { PH: i.title })}><Icon name="hourglass-split" />7 {t('m.timeline.days')}</button>
            </div>
          </li>
        {:else}
          <li class="d2-muted none">{t('m.timeline.no_deadlines_in_next')}</li>
        {/each}
      {/if}
    </ul>
  {/if}
</Widget>

<style>
  .sub {
    font-size: var(--d2-fs-xs);
  }
  .err {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
  }
  .cards {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s2);
    max-height: 420px;
    margin: 0;
    padding: 0;
    list-style: none;
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .card {
    flex: 1 1 220px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-top: 3px solid var(--d2-border-strong);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  .card.medium {
    border-top-color: var(--d2-info);
  }
  .card.high {
    border-top-color: var(--d2-warn);
  }
  .card.over {
    border-top-color: var(--d2-danger);
  }
  .when {
    display: flex;
    justify-content: space-between;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-xs);
  }
  .left {
    font-weight: 700;
  }
  .over .left {
    color: var(--d2-danger);
  }
  .ttl {
    padding: 0;
    border: 0;
    background: none;
    color: var(--d2-text);
    font-weight: 600;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    cursor: pointer;
  }
  .ttl:hover {
    color: var(--d2-accent-text);
    text-decoration: underline;
  }
  .meta {
    display: flex;
    justify-content: space-between;
    color: var(--d2-text-2);
    font-size: var(--d2-fs-xs);
  }
  .acts {
    display: flex;
    gap: 4px;
    margin-top: 4px;
  }
  .none {
    padding: var(--d2-s2) 0;
  }
</style>
