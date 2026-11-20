<!-- Overview of the workflows (topic 1 A): switch, name, one sentence from the short form, "n× this week", last run,
     marker on errors; order by dragging or keyboard (Alt+↑/↓ on the handle); New (gallery), duplicate,
     delete with confirmation. Data: GET /rs/api/v2/automation/workflows. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import Gallery from './Gallery.svelte'
  import { createWorkflow, deleteWorkflow, errorOf, getWorkflow, listWorkflows, patchWorkflow, type ListItem } from './api'
  import { ago, draftFromCombined, duplicateDraft, moveItem, reorderPlan, toSaveBody } from './model'
  import { sentenceText } from './sentence'

  let items = $state<ListItem[] | null>(null)
  let failed = $state(false)
  let menuFor = $state<number | null>(null)
  let confirmDel = $state<ListItem | null>(null)
  let busy = $state(false)
  let galleryOpen = $state(false)
  // "New workflow" from the command palette (?new=1): open the gallery, remove the parameter
  $effect(() => {
    if (router.query.includes('new=1') && canAdd) {
      galleryOpen = true
      router.navigate('automation/workflows', true)
    }
  })
  const canAdd = session.can('add', 'workflow')
  const canChange = session.can('change', 'workflow')
  const canDelete = session.can('delete', 'workflow')
  const loc = locale()

  async function load() {
    failed = false
    try {
      items = (await listWorkflows()).items
    } catch {
      failed = true
    }
  }
  onMount(load)

  async function toggle(w: ListItem) {
    const next = !w.enabled
    w.enabled = next
    try {
      await patchWorkflow(w.id, { enabled: next })
      toastService.show({ content: t(next ? 'm.auto.toast_enabled' : 'm.auto.toast_disabled', { PH: w.name }), delay: 3000 })
    } catch (e) {
      w.enabled = !next
      toastService.showError(t('m.auto.could_not_save'), e)
    }
  }

  async function duplicate(w: ListItem) {
    menuFor = null
    busy = true
    try {
      const c = await getWorkflow(w.id)
      const d = duplicateDraft(draftFromCombined(c), t('m.auto.copy_name', { PH: w.name }))
      d.workflow.order = (items?.reduce((m, x) => Math.max(m, x.order), 0) ?? 0) + 10
      const created = await createWorkflow(toSaveBody(d))
      toastService.show({ content: t('m.auto.duplicated', { PH: created.workflow.name }), delay: 4000, actionName: t('common.edit'), action: () => router.navigate(`automation/workflows/${created.workflow.id}`) })
      await load()
    } catch (e) {
      const err = errorOf(e)
      toastService.showError(err.status === 403 ? t('m.auto.not_allowed') : t('m.auto.could_not_save'), e)
    } finally {
      busy = false
    }
  }

  async function remove() {
    const w = confirmDel
    if (!w) return
    busy = true
    try {
      await deleteWorkflow(w.id)
      toastService.show({ content: t('m.auto.deleted', { PH: w.name }), delay: 4000 })
      confirmDel = null
      await load()
    } catch (e) {
      toastService.showError(t('m.auto.could_not_delete'), e)
    } finally {
      busy = false
    }
  }

  // ---- Order -------------------------------------------------------------------------------------
  let dragFrom = $state<number | null>(null)
  let dropAt = $state<{ index: number; after: boolean } | null>(null)
  async function applyOrder(from: number, to: number) {
    if (!items || from === to) return
    const plan = reorderPlan(items, from, to)
    const before = items
    items = moveItem(items, from, to).map((x) => ({ ...x, order: plan.find((p) => p.id === x.id)?.order ?? x.order }))
    try {
      for (const p of plan) await patchWorkflow(p.id, { order: p.order })
    } catch (e) {
      items = before
      toastService.showError(t('m.auto.could_not_save'), e)
      await load()
    }
  }
  function ondragover(e: DragEvent, i: number) {
    if (dragFrom === null) return
    e.preventDefault()
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
    dropAt = { index: i, after: e.clientY > r.top + r.height / 2 }
  }
  function ondrop(e: DragEvent) {
    e.preventDefault()
    if (dragFrom === null || !dropAt) return
    let to = dropAt.index + (dropAt.after ? 1 : 0)
    if (to > dragFrom) to--
    const from = dragFrom
    dragFrom = null
    dropAt = null
    void applyOrder(from, to)
  }
  function gripKey(e: KeyboardEvent, i: number) {
    if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return
    e.preventDefault()
    const to = e.key === 'ArrowUp' ? i - 1 : i + 1
    if (!items || to < 0 || to >= items.length) return
    void applyOrder(i, to).then(() => document.getElementById(`au-grip-${items?.[to]?.id}`)?.focus())
  }

  const lastRunText = (w: ListItem) => (w.last_run ? ago(w.last_run.at, loc) : t('m.auto.never_ran'))
</script>

<div class="au-page">
  <div class="au-head">
    <h1>{t('common.workflows')}</h1>
    {#if canAdd}
      <button type="button" class="d2-btn primary" id="au-new" onclick={() => (galleryOpen = true)}><Icon name="plus-lg" />{t('m.auto.new_workflow')}</button>
    {/if}
  </div>
  <div class="au-body d2-scroll au-cq">
    <p class="au-lead">{t('m.auto.list_intro')}</p>
    {#if failed}
      <div class="au-banner danger" role="alert">{t('m.auto.load_failed')} <button type="button" class="d2-btn sm" onclick={load}>{t('m.auto.retry')}</button></div>
    {:else if items === null}
      <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
    {:else if !items.length}
      <div class="au-empty">
        <p>{t('m.auto.list_empty')}</p>
        {#if canAdd}<button type="button" class="d2-btn primary" onclick={() => (galleryOpen = true)}><Icon name="plus-lg" />{t('m.auto.new_workflow')}</button>{/if}
      </div>
    {:else}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <ul class="au-list" id="au-workflow-list" aria-label={t('common.workflows')} ondrop={ondrop} ondragover={(e) => dragFrom !== null && e.preventDefault()}>
        {#each items as w, i (w.id)}
          {@const err = w.last_run?.result === 'error'}
          <li
            class="au-row"
            class:off={!w.enabled}
            class:dragging={dragFrom === i}
            class:drop-before={dropAt?.index === i && !dropAt.after}
            class:drop-after={dropAt?.index === i && dropAt.after}
            data-id={w.id}
            ondragover={(e) => ondragover(e, i)}
          >
            {#if canChange}
              <button
                type="button"
                class="au-grip grip"
                id="au-grip-{w.id}"
                draggable="true"
                aria-label={t('m.auto.reorder_hint', { PH: w.name })}
                title={t('m.auto.reorder_hint', { PH: w.name })}
                ondragstart={(e) => {
                  dragFrom = i
                  e.dataTransfer?.setData('text/plain', String(w.id))
                }}
                ondragend={() => {
                  dragFrom = null
                  dropAt = null
                }}
                onkeydown={(e) => gripKey(e, i)}><Icon name="grip-vertical" /></button
              >
            {:else}<span></span>{/if}
            <button type="button" class="au-switch" role="switch" aria-checked={w.enabled} aria-label={t('m.auto.active_label', { PH: w.name })} disabled={!canChange} onclick={() => toggle(w)}></button>
            <div class="main">
              <a class="name" href={router.href(`automation/workflows/${w.id}`)}>
                {w.name}
                {#if err}<span class="au-st error"><Icon name="exclamation-triangle" />{t('m.auto.has_error')}</span>{/if}
                {#if w.has_else}<span class="au-st skipped">{t('m.auto.with_else')}</span>{/if}
              </a>
              <p class="sent">{sentenceText(w.summary, t, loc)}</p>
            </div>
            <div class="meta">
              <strong>{t('m.auto.runs_this_week', { N: w.runs_this_week })}</strong>
              <span title={w.last_run ? t(err ? 'm.auto.last_run_error' : 'm.auto.last_run') : undefined}>{lastRunText(w)}</span>
            </div>
            <div class="au-adder">
              <button type="button" class="d2-btn ghost icon" aria-label={t('m.auto.more_for', { PH: w.name })} aria-haspopup="menu" aria-expanded={menuFor === w.id} onclick={() => (menuFor = menuFor === w.id ? null : w.id)}><Icon name="three-dots" /></button>
              {#if menuFor === w.id}
                <Menu label={t('m.auto.more_for', { PH: w.name })} onclose={() => (menuFor = null)}>
                  <a class="d2-menu-item" role="menuitem" href={router.href(`automation/workflows/${w.id}`)}><Icon name="pencil" />{t('common.edit')}</a>
                  <button type="button" class="d2-menu-item" role="menuitem" onclick={() => ((menuFor = null), router.navigate(`automation/workflows/${w.id}?tab=runs`))}><Icon name="clock-history" />{t('m.auto.history')}</button>
                  {#if canAdd}<button type="button" class="d2-menu-item" role="menuitem" disabled={busy} onclick={() => duplicate(w)}><Icon name="copy" />{t('m.auto.duplicate')}</button>{/if}
                  {#if canChange && i > 0}<button type="button" class="d2-menu-item" role="menuitem" onclick={() => ((menuFor = null), applyOrder(i, i - 1))}><Icon name="arrow-up" />{t('m.auto.move_up')}</button>{/if}
                  {#if canChange && i < items.length - 1}<button type="button" class="d2-menu-item" role="menuitem" onclick={() => ((menuFor = null), applyOrder(i, i + 1))}><Icon name="arrow-down" />{t('m.auto.move_down')}</button>{/if}
                  {#if canDelete}
                    <div class="d2-menu-sep"></div>
                    <button type="button" class="d2-menu-item" role="menuitem" onclick={() => ((menuFor = null), (confirmDel = w))}><Icon name="trash" />{t('common.delete')}</button>
                  {/if}
                </Menu>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
      <p class="au-hint au-mt2">{t('m.auto.order_hint')}</p>
    {/if}
  </div>
</div>

{#if galleryOpen}
  <Gallery onclose={() => (galleryOpen = false)} />
{/if}
{#if confirmDel}
  <ConfirmDialog title={t('m.auto.delete_title', { PH: confirmDel.name })} confirmLabel={t('common.delete')} danger {busy} onconfirm={remove} oncancel={() => (confirmDel = null)}>
    <p>{t('m.auto.delete_text')}</p>
  </ConfirmDialog>
{/if}
