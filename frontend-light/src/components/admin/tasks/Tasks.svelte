<!-- Port of admin/tasks/tasks.component.html (route tasks). Host <pngx-tasks> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/tasks.css'
  import { onDestroy } from 'svelte'
  import { SvelteSet } from 'svelte/reactivity'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { ricon } from '../../../lib/route-icons'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { Popover } from '../../../lib/popover.svelte'
  import { formatDate } from '../../../lib/dates'
  import { modals } from '../../../lib/modal.svelte'
  import { router } from '../../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { api } from '../../../api/client'
  import type { Paged } from '../../../api/v1'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import Pagination from '../../common/pagination/Pagination.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import './tasks-icons'
  import {
    SECTIONS,
    SECTION_LABELS,
    FILTER_TARGETS,
    TASK_TYPE_OPTIONS,
    TRIGGER_SOURCE_OPTIONS,
    taskTypeLabel,
    triggerSourceLabel,
    duplicateDocumentId,
    json,
    paramsForSection,
    query,
    taskBelongsToSection,
    taskDisplayName,
    taskMatchesFilters,
    taskResultMessage,
    taskResultPreview,
    type PaperlessTask,
    type TaskSection,
  } from './tasks'

  const pageSize = 25
  const overviewSize = 10
  const selectedTasks = new SvelteSet<number>()
  let expandedTask = $state<number | undefined>(undefined)
  let autoRefreshEnabled = $state(true)
  let page = $state(1)
  let totalTasks = $state(0)
  let sectionCounts = $state<Record<TaskSection, number>>({ all: 0, needs_attention: 0, in_progress: 0, completed: 0 })
  let pagedTasks = $state<PaperlessTask[]>([])
  let loading = $state(true)
  let selectedSection = $state<TaskSection>('all')
  let selectedTaskType = $state<string | null>(null)
  let selectedTriggerSource = $state<string | null>(null)
  let filterText = $state('')
  let filterInput = $state('')
  let filterTargetID = $state(0)

  const filters = () => ({ taskType: selectedTaskType, triggerSource: selectedTriggerSource, filterText, filterTargetID })
  const filterTargetName = $derived(t(FILTER_TARGETS.find((x) => x.id == filterTargetID)!.name))
  const selectedTaskTypeLabel = $derived(selectedTaskType === null ? t('admin.tasks.all_types') : t(TASK_TYPE_OPTIONS.find((o) => o.value === selectedTaskType)?.label ?? selectedTaskType))
  const selectedTriggerSourceLabel = $derived(selectedTriggerSource === null ? t('admin.tasks.all_sources') : t(TRIGGER_SOURCE_OPTIONS.find((o) => o.value === selectedTriggerSource)?.label ?? selectedTriggerSource))
  const dismissButtonText = $derived(selectedTasks.size > 0 ? t('admin.tasks.dismiss_selected') : t('admin.tasks.dismiss_visible'))
  const tasksForSection = (section: TaskSection) => pagedTasks.filter((task) => taskBelongsToSection(task, section) && taskMatchesFilters(task, filters()))
  const visibleSections = $derived((selectedSection === 'all' ? SECTIONS : [selectedSection]).filter((s) => tasksForSection(s).length > 0))
  const visibleTasks = $derived(visibleSections.flatMap((s) => tasksForSection(s)))
  const isFiltered = $derived(selectedTaskType !== null || selectedTriggerSource !== null || filterText.length > 0)
  const canChange = session.can('change', 'paperlesstask')
  const canViewDocs = session.can('view', 'document')

  function optionCount(over: { taskType?: string | null; triggerSource?: string | null }) {
    const sections = selectedSection === 'all' ? SECTIONS : [selectedSection]
    const f = { ...filters(), ...over }
    return pagedTasks.filter((task) => sections.some((s) => taskBelongsToSection(task, s)) && taskMatchesFilters(task, f)).length
  }

  const list = (pg: number, size: number, section: TaskSection) =>
    api.get<Paged<PaperlessTask>>(`/api/tasks/${query([['page', String(pg)], ['page_size', String(size)], ...paramsForSection(section, filters())])}`)

  async function reloadSectionCounts() {
    try {
      const c = await api.get<any>(`/api/tasks/status_counts/${query(paramsForSection('all', filters()))}`)
      sectionCounts = { all: c.all, needs_attention: c.needs_attention, in_progress: c.in_progress, completed: c.completed }
    } catch {}
  }

  /** TasksService.reload: sidebar counter */
  function reloadService() {
    api.taskStatusCounts().then((c: any) => (data.needsAttentionTasks = c?.needs_attention ?? 0)).catch(() => {})
  }

  let seq = 0
  async function reloadPage(resetToFirstPage = false) {
    if (resetToFirstPage) page = 1
    reloadSectionCounts()
    loading = true
    const n = ++seq
    try {
      let result: { count: number; results: PaperlessTask[] }
      if (selectedSection !== 'all') result = await list(page, pageSize, selectedSection)
      else {
        const rs = await Promise.all(SECTIONS.map((s) => list(1, overviewSize, s)))
        result = { count: rs.reduce((sum, r) => sum + r.count, 0), results: rs.flatMap((r) => r.results) }
      }
      if (n !== seq) return
      pagedTasks = result.results
      totalTasks = result.count
      loading = false
      if (page > 1 && pagedTasks.length === 0 && totalTasks > 0) {
        page -= 1
        reloadPage()
      }
    } catch {
      loading = false
    }
  }

  reloadService()
  reloadPage()
  const timer = setInterval(() => {
    if (!autoRefreshEnabled) return
    reloadService()
    reloadPage(false)
  }, 5000)
  let filterDebounce: ReturnType<typeof setTimeout> | undefined
  onDestroy(() => {
    clearInterval(timer)
    clearTimeout(filterDebounce)
  })

  /** filterText setter: debounceTime(100), distinctUntilChanged, length 0 or > 2 */
  let lastQuery = ''
  function onFilterInput() {
    clearTimeout(filterDebounce)
    const q = filterInput
    filterDebounce = setTimeout(() => {
      if (q === lastQuery || (q.length && q.length <= 2)) return
      lastQuery = q
      filterText = q
      clearSelection()
      reloadPage(true)
    }, 100)
  }
  function filterInputKeyup(e: KeyboardEvent) {
    if (e.key == 'Enter') {
      filterText = (e.target as HTMLInputElement).value
      clearSelection()
      reloadPage(true)
    } else if (e.key === 'Escape') resetFilter()
  }
  function resetFilter() {
    if (!filterText.length) return
    filterText = ''
    filterInput = ''
    clearSelection()
    reloadPage(true)
  }
  function resetFilters() {
    const had = isFiltered
    selectedTaskType = null
    selectedTriggerSource = null
    filterText = ''
    filterInput = ''
    clearSelection()
    if (had) reloadPage(true)
  }
  function setSection(s: TaskSection) {
    selectedSection = s
    clearSelection()
    reloadPage(true)
  }
  function setTaskType(v: string | null) {
    selectedTaskType = v
    clearSelection()
    reloadPage(true)
  }
  function setTriggerSource(v: string | null) {
    selectedTriggerSource = v
    clearSelection()
    reloadPage(true)
  }
  function setFilterTarget(id: number) {
    filterTargetID = id
    if (filterText.length) {
      clearSelection()
      reloadPage(true)
    }
  }
  function setPage(p: number) {
    if (page === p) return
    page = p
    clearSelection()
    reloadPage()
  }
  const clearSelection = () => selectedTasks.clear()
  function toggleSelected(task: PaperlessTask) {
    if (selectedTasks.has(task.id)) selectedTasks.delete(task.id)
    else selectedTasks.add(task.id)
  }
  function toggleSection(section: TaskSection, e: Event) {
    const checked = (e.target as HTMLInputElement).checked
    for (const task of tasksForSection(section)) checked ? selectedTasks.add(task.id) : selectedTasks.delete(task.id)
  }
  const areAllSelected = (tasks: PaperlessTask[]) => tasks.length > 0 && tasks.every((x) => selectedTasks.has(x.id))
  const expandTask = (task: PaperlessTask) => (expandedTask = expandedTask == task.id ? undefined : task.id)

  async function acknowledge(body: any) {
    await api.request('POST', '/api/tasks/acknowledge/', body)
    reloadService()
  }

  function dismissTasks(task?: PaperlessTask) {
    let tasks = task ? new Set([task.id]) : new Set(selectedTasks)
    if (!task && tasks.size == 0) tasks = new Set(visibleTasks.map((x) => x.id))
    if (tasks.size > 1) {
      const ref = modals.open<string>(
        ConfirmDialog,
        {
          title: t('admin.tasks.confirm_dismiss'),
          messageBold: t('admin.tasks.dismiss_tasks', { PH: tasks.size }),
          btnClass: 'btn-warning',
          btnCaption: t('common.dismiss'),
          onconfirm: (setEnabled: (b: boolean) => void) => {
            setEnabled(false)
            ref.close()
            acknowledge({ tasks: [...tasks] })
              .then(() => reloadPage(false))
              .catch((e) => {
                toastService.showError(t('admin.tasks.error_dismissing_tasks'), e)
                setEnabled(true)
              })
            clearSelection()
          },
        },
        { backdrop: 'static' },
      )
    } else if (tasks.size === 1) {
      acknowledge({ tasks: [...tasks] })
        .then(() => reloadPage(false))
        .catch((e) => toastService.showError(t('admin.tasks.error_dismissing_task'), e))
      clearSelection()
    }
  }

  function dismissAllTasks() {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('admin.tasks.confirm_dismiss_all'),
        messageBold: t('admin.tasks.dismiss_all_tasks', { PH: totalTasks }),
        btnClass: 'btn-warning',
        btnCaption: t('common.dismiss'),
        onconfirm: (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          ref.close()
          acknowledge({ all: true })
            .then(() => reloadPage(false))
            .catch((e) => {
              toastService.showError(t('admin.tasks.error_dismissing_tasks'), e)
              setEnabled(true)
            })
          clearSelection()
        },
      },
      { backdrop: 'static' },
    )
  }

  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const ddType = new Dropdown()
  const ddSource = new Dropdown()
  const ddTarget = new Dropdown()
  const pops = new Map<number, Popover>()
  const pop = (id: number) => {
    let p = pops.get(id)
    if (!p) pops.set(id, (p = new Popover({ triggers: 'mouseenter:mouseleave' })))
    return p
  }
</script>

{#snippet tasksTemplate(tasks: PaperlessTask[], section: TaskSection)}
  {@const count = sectionCounts[section]}
  {@const hasMore = selectedSection === 'all' && count > tasks.length}
  <div class="section-header mb-2">
    <h5 class="mb-0">{t(SECTION_LABELS[section])}</h5>
    <div class="small text-muted">
      {#if hasMore}
        {t('admin.tasks.tasks', { INTERPOLATION: tasks.length, INTERPOLATION_1: count })}
        <button class="btn btn-link show-all p-0 ms-2 align-baseline" type="button" onclick={() => setSection(section)}>
          {t('common.show_all')}<pngx-bs-icon class="ms-1" width="1em" height="1em" name="arrow-right">{@html ricon('arrow-right', '1em', '1em')}</pngx-bs-icon>
        </button>
      {:else}
        {t('admin.tasks.tasks_2', { VAR_PLURAL: count, INTERPOLATION: count })}
      {/if}
    </div>
  </div>

  <div class="card border table-responsive mb-3">
    <table class="table table-striped align-middle shadow-sm mb-0 tasks-table">
      <thead>
        <tr>
          <th scope="col" class="select-col">
            <div class="form-check">
              <input
                type="checkbox"
                class="form-check-input"
                id={'all-tasks-' + section}
                disabled={tasks.length === 0}
                checked={areAllSelected(tasks)}
                onclick={(e) => { toggleSection(section, e); e.stopPropagation() }} />
              <label class="form-check-label" for="all-tasks-{section}"><span class="visually-hidden">Check all</span></label>
            </div>
          </th>
          <th scope="col" class="name-col-header">{t('common.name')}</th>
          <th scope="col" class="d-none d-lg-table-cell created-col">{t('common.created')}</th>
          {#if section !== 'in_progress'}
            <th scope="col" class="d-none d-lg-table-cell results-col">{t('common.results')}</th>
          {/if}
          <th scope="col" class="d-table-cell d-lg-none">{t('admin.tasks.info')}</th>
          <th scope="col" class="actions-col">{t('common.actions')}</th>
        </tr>
      </thead>
      <tbody>
        {#each tasks as task (task.id)}
          {@const msgText = taskResultMessage(task)}
          {@const dup = duplicateDocumentId(task)}
          <tr onclick={(e) => { toggleSelected(task); e.stopPropagation() }}>
            <td class="select-col">
              <div class="form-check">
                <input
                  type="checkbox"
                  class="form-check-input"
                  id="task{task.id}"
                  checked={selectedTasks.has(task.id)}
                  onclick={(e) => { toggleSelected(task); e.stopPropagation() }} />
                <label class="form-check-label" for="task{task.id}"></label>
              </div>
            </td>
            <td class="overflow-auto name-col">
              <div>{taskDisplayName(task)}</div>
              <div class="small text-muted">
                {#if taskDisplayName(task) !== taskTypeLabel(task)}
                  <span class="me-2">{taskTypeLabel(task)}</span>
                {/if}
                <span class="text-nowrap"><pngx-bs-icon class="me-1" width="1em" height="1em" name="box-arrow-in-right">{@html ricon('box-arrow-in-right', '1em', '1em')}</pngx-bs-icon>{triggerSourceLabel(task)}</span>
              </div>
            </td>
            <td class="d-none d-lg-table-cell created-col">{formatDate(task.date_created, 'short', dateLocale)}</td>
            {#if section !== 'in_progress'}
              <td class="d-none d-lg-table-cell results-col">
                {#if (msgText?.length ?? 0) > 50}
                  {@const p = pop(task.id)}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
                  <div class="result" onclick={(e) => { expandTask(task); e.stopPropagation() }} use:p.triggerEl>
                    <span class="small d-none d-md-inline-block font-monospace text-muted">{taskResultPreview(task)}</span>
                  </div>
                  {#if p.open}
                    <ngb-popover-window role="tooltip" class="popover shadow small mobile" id={p.id} use:p.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
                      <pre class="small mb-0">{msgText?.slice(0, 300)}{#if (msgText?.length ?? 0) > 300}
                        &hellip;
                      {/if}</pre>
                      {#if (msgText?.length ?? 0) > 300}
                        <br/><em>({t('admin.tasks.click_full_output')})</em>
                      {/if}
                    </div></ngb-popover-window>
                  {/if}
                {/if}
                {#if msgText && msgText.length <= 50}
                  <span class="small d-none d-md-inline-block font-monospace text-muted">{msgText}</span>
                {/if}
                {#if dup}
                  <div class="small text-warning-emphasis d-flex align-items-center gap-1 mt-1">
                    <pngx-bs-icon class="lh-1" width="1em" height="1em" name="exclamation-triangle">{@html ricon('exclamation-triangle', '1em', '1em')}</pngx-bs-icon>
                    <span>{t('admin.tasks.duplicate_document', { PH: dup })}</span>
                  </div>
                {/if}
              </td>
            {/if}
            <td class="d-lg-none">
              <button class="btn btn-link" onclick={(e) => { expandTask(task); e.stopPropagation() }} aria-label={t('admin.tasks.view_task_details')}>
                <pngx-bs-icon width="1.2em" height="1.2em" name="info-circle">{@html icon('info-circle', '1.2em', '1.2em')}</pngx-bs-icon>
              </button>
            </td>
            <td class="actions-col">
              <div class="btn-group" role="group">
                {#if canChange}
                  <button class="btn btn-sm btn-outline-secondary" onclick={(e) => { dismissTasks(task); e.stopPropagation() }}>
                    <pngx-bs-icon name="check" class="me-1">{@html icon('check')}</pngx-bs-icon>{t('common.dismiss')}
                  </button>
                {/if}
                {#if canViewDocs}
                  {#if task.related_document_ids?.[0]}
                    <a class="btn btn-sm btn-outline-primary" href={router.href(`documents/${task.related_document_ids[0]}`)} onclick={() => dismissTasks(task)}>
                      <pngx-bs-icon name="file-text" class="me-1">{@html ricon('file-text')}</pngx-bs-icon>{t('common.open_document')}
                    </a>
                  {/if}
                {/if}
              </div>
            </td>
          </tr>
        <tr>
          <td class="px-2 py-0" class:border-0={expandedTask !== task.id} colspan={section !== 'in_progress' ? 5 : 4}>
            <div class="task-detail-panel bg-darker small mb-0 collapse" class:show={expandedTask === task.id}>
              <div class="p-2 p-lg-3 ms-lg-3">
                {#if msgText}
                  <div class="detail-section mb-3">
                    <div class="detail-label fs-7 fw-bold text-uppercase text-muted mb-1">{t('admin.tasks.result_message')}</div>
                    <pre class="detail-block border border-dark bg-body p-3 rounded-2 mb-0">{msgText}</pre>
                  </div>
                {/if}

                {#if dup}
                  <div class="detail-section mb-3">
                    <div class="detail-label fs-7 fw-bold text-uppercase text-muted mb-1">{t('admin.tasks.duplicate')}</div>
                    <div class="detail-block border border-dark bg-body p-3 rounded-2 mb-0">
                      <div class="d-flex align-items-center justify-content-between gap-3">
                        <div class="text-break">{t('admin.tasks.duplicate_document', { PH: dup })}</div>
                        <button
                          class="btn btn-sm btn-outline-primary"
                          type="button"
                          onclick={() => router.navigate(`documents/${dup}/details`)}>{t('common.open')}</button>
                      </div>
                    </div>
                  </div>
                {/if}

                <div class="row g-3">
                  <div class="col-12 col-xl-6">
                    <div class="detail-section h-100">
                      <div class="detail-label fs-7 fw-bold text-uppercase text-muted mb-1">{t('admin.tasks.input_data')}</div>
                      <pre class="detail-block border border-dark bg-body p-3 rounded-2 mb-0">{json(task.input_data)}</pre>
                    </div>
                  </div>
                  <div class="col-12 col-xl-6">
                    <div class="detail-section h-100">
                      <div class="detail-label fs-7 fw-bold text-uppercase text-muted mb-1">{t('admin.tasks.result_data')}</div>
                      <pre class="detail-block border border-dark bg-body p-3 rounded-2 mb-0">{json(task.result_data ?? {})}</pre>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </td>
        </tr>
      {/each}
      </tbody>
    </table>
  </div>
{/snippet}

<pngx-page-header>
<PageHeader
  title={t('common.tasks')}
  info={t('admin.tasks.tasks_shows_detailed_information')}
  loading={loading && pagedTasks.length === 0}
  >
  <div class="btn-toolbar col col-md-auto align-items-center gap-2">
    <button class="btn btn-sm btn-outline-secondary me-2" onclick={clearSelection} hidden={selectedTasks.size === 0}>
      <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('common.clear_selection')}
    </button>
    {#if canChange}
      <button class="btn btn-sm btn-outline-primary me-2" onclick={() => dismissTasks()} disabled={visibleTasks.length === 0}>
        <pngx-bs-icon name="check2-all" class="me-1">{@html icon('check2-all')}</pngx-bs-icon>{dismissButtonText}
      </button>
    {/if}
    {#if canChange}
      <button class="btn btn-sm btn-outline-primary me-2" onclick={dismissAllTasks} disabled={totalTasks === 0}>
        <pngx-bs-icon name="check2-all" class="me-1">{@html icon('check2-all')}</pngx-bs-icon>{t('common.dismiss_all')}
      </button>
    {/if}
    <div class="form-check form-switch mb-0 ms-2">
      <input id="autoRefreshSwitch" class="form-check-input" type="checkbox" role="switch" bind:checked={autoRefreshEnabled}>
      <label class="form-check-label" for="autoRefreshSwitch">{t('common.auto_refresh')}</label>
    </div>
  </div>
</PageHeader>
</pngx-page-header>

<div class="task-controls mb-3 gap-3 btn-toolbar align-items-center" role="toolbar">
  <div class="task-view-scope btn-group btn-group-sm" role="group">
    <input
      type="radio"
      class="btn-check"
      checked={selectedSection === 'all'}
      id="section-all"
      onclick={() => setSection('all')} />
    <label class="btn btn-outline-primary" for="section-all">
      {t('common.all')}
    </label>
    {#each SECTIONS as section (section)}
      <input
        type="radio"
        class="btn-check"
        checked={selectedSection === section}
        id="section-{section}"
        onclick={() => setSection(section)} />
      <label class="btn btn-outline-primary d-flex flex-row align-items-center" for="section-{section}">
        {t(SECTION_LABELS[section])}
        {#if sectionCounts[section] > 0}
          <span class="badge ms-2" class:bg-danger={section === 'needs_attention'} class:bg-secondary={section !== 'needs_attention'}>{sectionCounts[section]}</span>
        {/if}
      </label>
    {/each}
  </div>

  <div class="d-flex align-items-center gap-2">
    <div class="text-muted">{t('admin.tasks.filter')}:</div>

    <div class="dropdown" class:show={ddType.open} use:ddType.hostEl>
      <button class="btn btn-sm btn-outline-primary dropdown-toggle" aria-expanded={ddType.open} onclick={ddType.toggle} use:ddType.anchorEl>{selectedTaskTypeLabel}</button>
      {#if ddType.open}
        <div class="dropdown-menu shadow show" use:ddType.menuEl>
          <button class="dropdown-item" class:active={selectedTaskType === null} onclick={() => setTaskType(null)}>{t('admin.tasks.all_types')}</button>
          {#each TASK_TYPE_OPTIONS as option (option.value)}
            <button class="dropdown-item" class:active={selectedTaskType === option.value} disabled={optionCount({ taskType: option.value }) === 0} onclick={() => setTaskType(option.value)}>{t(option.label)}</button>
          {/each}
        </div>
      {/if}
    </div>
    <div class="dropdown" class:show={ddSource.open} use:ddSource.hostEl>
      <button class="btn btn-sm btn-outline-primary dropdown-toggle" aria-expanded={ddSource.open} onclick={ddSource.toggle} use:ddSource.anchorEl>{selectedTriggerSourceLabel}</button>
      {#if ddSource.open}
        <div class="dropdown-menu shadow show" use:ddSource.menuEl>
          <button class="dropdown-item" class:active={selectedTriggerSource === null} onclick={() => setTriggerSource(null)}>{t('admin.tasks.all_sources')}</button>
          {#each TRIGGER_SOURCE_OPTIONS as option (option.value)}
            <button class="dropdown-item" class:active={selectedTriggerSource === option.value} disabled={optionCount({ triggerSource: option.value }) === 0} onclick={() => setTriggerSource(option.value)}>{t(option.label)}</button>
          {/each}
        </div>
      {/if}
    </div>
  </div>

  <div class="form-inline d-flex align-items-center flex-grow-1 task-search">
    <div class="input-group input-group-sm flex-fill w-auto flex-nowrap">
      <div class="dropdown" class:show={ddTarget.open} use:ddTarget.hostEl>
        <button class="btn btn-sm btn-outline-primary dropdown-toggle" aria-expanded={ddTarget.open} onclick={ddTarget.toggle} use:ddTarget.anchorEl>{filterTargetName}</button>
        {#if ddTarget.open}
          <div class="dropdown-menu shadow show" use:ddTarget.menuEl>
            {#each FILTER_TARGETS as ft (ft.id)}
              <button class="dropdown-item" class:active={filterTargetID === ft.id} onclick={() => setFilterTarget(ft.id)}>{t(ft.name)}</button>
            {/each}
          </div>
        {/if}
      </div>
      {#if filterText?.length}
        <button class="btn btn-link btn-sm px-2 position-absolute top-0 end-0 z-10" onclick={resetFilter} aria-label={t('common.clear_search')}>
          <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon>
        </button>
      {/if}
      <label class="visually-hidden" for="taskSearch">{t('admin.tasks.search_tasks')}</label>
      <input id="taskSearch" class="form-control form-control-sm" type="text"
        onkeyup={filterInputKeyup}
        bind:value={filterInput}
        oninput={onFilterInput}>
    </div>
  </div>

  {#if isFiltered}
    <button class="btn btn-link py-0 ms-md-auto" onclick={resetFilters}>
      <pngx-bs-icon width="1em" height="1em" name="x">{@html icon('x', '1em', '1em')}</pngx-bs-icon><small>{t('common.reset_filters')}</small>
    </button>
  {/if}

  {#if selectedSection !== 'all' && totalTasks > pageSize}
    <ngb-pagination size="sm" aria-label="Tasks pagination" role="navigation"><Pagination {pageSize} collectionSize={totalTasks} {page} maxSize={5} rotate={true} onpage={setPage} /></ngb-pagination>
  {/if}
</div>

{#if visibleSections.length > 0}
  {#each visibleSections as section (section)}
    <div class="mb-4">
      {@render tasksTemplate(tasksForSection(section), section)}
    </div>
  {/each}
{:else if !loading || pagedTasks.length > 0}
  <div class="alert alert-secondary fst-italic">{t('admin.tasks.no_tasks_match_current')}</div>
{/if}
