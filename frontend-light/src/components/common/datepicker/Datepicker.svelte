<!-- Replica of the ngbDatepicker popup (ng-bootstrap 21, NgbInputDatepicker.open): <ngb-datepicker class="dropdown-menu show">
     directly after the input field (like ViewContainerRef of the input field), Popper bottom-start/-end, top-start/-end,
     offset [0, 2]; header with month/year selection and arrows, one month with 6 weeks, footer = footer snippet of the
     parent component (encapsulated there like the Angular ng-template). Closing like ngbAutoClose (click outside
     input field and popup, Escape) and after selecting a day. Loaded only on demand (lazy.ts). -->
<script lang="ts">
  import '../../../styles/parts/datepicker.css'
  import { onMount, tick, type Snippet } from 'svelte'
  import { createPopperLite, flip, preventOverflow, arrow, offset } from '@popperjs/core'
  import { t, locale } from '../../../lib/i18n'
  import { isoFromModel, isoToModel, type DateStruct } from '../../../lib/date-input'
  import { buildMonth, firstDayOfWeek, getNext, getPrev, keyTarget, monthNames, before, selectYears, today as todayOf } from './datepicker'

  let {
    anchor,
    value = null,
    onselect,
    onclose,
    footer,
  }: {
    /** Input field (positioning, counts as "inside" for the close click) */
    anchor: HTMLElement | undefined
    /** Date as structure or ISO text (ISODateAdapter) */
    value?: DateStruct | string | null
    /** Selection as structure and as ISO text (yyyy-mm-dd) */
    onselect?: (d: DateStruct, iso: string) => void
    onclose?: () => void
    footer?: Snippet<[() => void]>
  } = $props()

  // Attribute selector [ngbDatepickerDayView] (vendor/ngb-datepicker.css)
  const DAY_VIEW: Record<string, string> = { ngbdatepickerdayview: '' }
  const loc = locale()
  const firstDay = firstDayOfWeek(loc)
  const monthsShort = monthNames(loc, 'short')
  const monthsFull = monthNames(loc, 'long')
  const now = todayOf()
  const selected = $derived(typeof value === 'string' ? isoFromModel(value) : value)
  // svelte-ignore state_referenced_locally
  const start = selected ?? now
  let view = $state({ year: start.year, month: start.month })
  let focus = $state<DateStruct>(start)
  let focusVisible = $state(false)
  const model = $derived(buildMonth(view.year, view.month, { locale: loc, firstDay, today: now, selected, focus, focusVisible }))
  const viewFirst = $derived({ year: view.year, month: view.month, day: 1 })
  const viewLast = $derived(getPrev(getNext(viewFirst, 'm', 1), 'd', 1))
  const years = $derived(selectYears(view.year))
  const monthLabel = $derived(`${monthsFull[view.month - 1]} ${view.year}`)

  let el: HTMLElement | undefined = $state()
  let content: HTMLElement | undefined = $state()
  let elWithFocus: Element | null = null

  /** NgbDatepickerService.open: different month → rebuild, focus into the visible area */
  function open(d: DateStruct) {
    view = { year: d.year, month: d.month }
    const first = { year: d.year, month: d.month, day: 1 }
    const last = getPrev(getNext(first, 'm', 1), 'd', 1)
    if (before(focus, first) || before(last, focus)) focus = d
  }
  async function focusDay() {
    await tick()
    content?.querySelector<HTMLElement>('div.ngb-dp-day[tabindex="0"]')?.focus()
  }
  function select(d: DateStruct) {
    focus = d
    onselect?.(d, isoToModel(d)!)
    close()
  }
  function close() {
    onclose?.()
  }
  function navigate(e: MouseEvent, n: number) {
    ;(e.currentTarget as HTMLElement).focus()
    open(getNext(viewFirst, 'm', n))
  }
  function onKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault()
      e.stopPropagation()
      select(focus)
      return
    }
    const target = keyTarget(e.key, e.shiftKey, focus, { first: viewFirst, last: viewLast })
    if (!target) return
    e.preventDefault()
    e.stopPropagation()
    focus = target
    if (before(target, viewFirst) || before(viewLast, target)) view = { year: target.year, month: target.month }
    focusDay()
  }

  onMount(() => {
    elWithFocus = document.activeElement
    let popper: ReturnType<typeof createPopperLite> | undefined
    if (anchor && el) {
      popper = createPopperLite(anchor, el, {
        placement: 'bottom-start',
        modifiers: [
          flip,
          preventOverflow,
          arrow,
          { enabled: true, name: 'flip', options: { fallbackPlacements: ['bottom-end', 'top-start', 'top-end'] } },
          offset,
          { name: 'offset', options: { offset: () => [0, 2] } },
        ] as any,
      })
    }
    setTimeout(() => focusDay())
    // ngbAutoClose(autoClose = true, ignoreElements = [input field, popup])
    const inside = (t: EventTarget | null) => !!t && ((anchor && anchor.contains(t as Node)) || (el && el.contains(t as Node)))
    let shouldClose = false
    const down = (e: MouseEvent) => (shouldClose = e.button !== 2 && !inside(e.target))
    const up = () => {
      if (shouldClose) setTimeout(close)
    }
    const key = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      }
    }
    document.addEventListener('mousedown', down)
    document.addEventListener('mouseup', up)
    document.addEventListener('keydown', key)
    return () => {
      document.removeEventListener('mousedown', down)
      document.removeEventListener('mouseup', up)
      document.removeEventListener('keydown', key)
      popper?.destroy()
      if (el?.contains(document.activeElement)) (elWithFocus as HTMLElement | null)?.focus?.()
    }
  })
</script>

<ngb-datepicker class="dropdown-menu show" bind:this={el}>
  <div class="ngb-dp-header">
    <ngb-datepicker-navigation>
      <div class="ngb-dp-arrow ngb-dp-arrow-prev">
        <button type="button" class="btn btn-link ngb-dp-arrow-btn" onclick={(e) => navigate(e, -1)} aria-label={t('common.previous_month')} title={t('common.previous_month')}>
          <span class="ngb-dp-navigation-chevron"></span>
        </button>
      </div>
      <ngb-datepicker-navigation-select class="ngb-dp-navigation-select">
        <select class="form-select" aria-label={t('ui.select_month')} title={t('ui.select_month')} value={view.month} onchange={(e) => open({ year: view.year, month: +e.currentTarget.value, day: 1 })}>
          {#each monthsShort as name, i (i)}
            <option aria-label={monthsFull[i]} value={i + 1}>{name}</option>
          {/each}
        </select>
        <select class="form-select" aria-label={t('ui.select_year')} title={t('ui.select_year')} value={view.year} onchange={(e) => open({ year: +e.currentTarget.value, month: view.month, day: 1 })}>
          {#each years as y (y)}
            <option value={y}>{y}</option>
          {/each}
        </select>
      </ngb-datepicker-navigation-select>
      <div class="visually-hidden" aria-live="polite">
        <span>{monthLabel}</span>
      </div>
      <div class="ngb-dp-arrow ngb-dp-arrow-next">
        <button type="button" class="btn btn-link ngb-dp-arrow-btn" onclick={(e) => navigate(e, 1)} aria-label={t('ui.next_month')} title={t('ui.next_month')}>
          <span class="ngb-dp-navigation-chevron"></span>
        </button>
      </div>
    </ngb-datepicker-navigation>
  </div>
  <div class="ngb-dp-content ngb-dp-months" bind:this={content} onfocusin={() => (focusVisible = true)} onfocusout={(e) => { if (!content?.contains(e.relatedTarget as Node)) focusVisible = false }}>
    <div class="ngb-dp-month">
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions, a11y_interactive_supports_focus -->
      <ngb-datepicker-month role="grid" onkeydown={onKeyDown}>
        <div class="ngb-dp-week ngb-dp-weekdays" role="row">
          {#each model.weekdays as wd, i (i)}
            <div class="ngb-dp-weekday small" role="columnheader">{wd}</div>
          {/each}
        </div>
        {#each model.weeks as week, w (w)}
          <div class="ngb-dp-week" role="row">
            {#each week as day (`${day.date.year}-${day.date.month}-${day.date.day}`)}
              <!-- svelte-ignore a11y_click_events_have_key_events -->
              <div class="ngb-dp-day" role="gridcell" class:ngb-dp-today={day.today} tabindex={day.tabindex} aria-label={day.ariaLabel} aria-disabled="false" aria-selected={day.selected} onclick={(e) => { e.preventDefault(); select(day.date) }}>
                <div {...DAY_VIEW} class="btn-light" class:bg-primary={day.selected} class:text-white={day.selected} class:text-muted={day.muted} class:outside={day.muted} class:active={day.focused}>{day.date.day}</div>
              </div>
            {/each}
          </div>
        {/each}
      </ngb-datepicker-month>
    </div>
  </div>
  {@render footer?.(close)}
</ngb-datepicker>
