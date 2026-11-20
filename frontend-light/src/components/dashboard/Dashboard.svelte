<!-- Port of dashboard/dashboard.component.html. Host <pngx-dashboard> in the parent markup (App.svelte).
     Reordering the widgets by dragging like cdkDropList/cdkDrag (lib/cdk-drag.ts), tour loaded lazily (lib/tour/). -->
<script lang="ts">
  import '../../styles/components/dashboard.css'
  import { t, tRich } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { setSetting, storeSettings } from '../../stores/settingsSave.svelte'
  import { cdkDropList, moveItemInArray } from '../../lib/cdk-drag'
  import { completeTour as completeTourSetting } from '../../lib/tour/complete'
  import { dropzone } from '../file-drop/dropzone.svelte'
  import { session, SETTINGS_KEYS, rawValue } from '../../stores/settings.svelte'
  import { data } from '../../stores/data.svelte'
  import { router } from '../../router.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import PageHeader from '../common/page-header/PageHeader.svelte'
  import WelcomeWidget from './widgets/welcome-widget/WelcomeWidget.svelte'
  import SavedViewWidget from './widgets/saved-view-widget/SavedViewWidget.svelte'
  import StatisticsWidget from './widgets/statistics-widget/StatisticsWidget.svelte'
  import UploadFileWidget from './widgets/upload-file-widget/UploadFileWidget.svelte'
  import { dashboardViews as sortDashboardViews, truthy, DASHBOARD_KEYS, type DashboardView } from './dashboard'

  const appTitle = $derived(session.get(SETTINGS_KEYS.APP_TITLE) || 'PaperRust')
  const subtitle = $derived(
    session.displayName ? t('dashboard.hello_welcome', { PH: session.displayName, PH_1: appTitle }) : t('dashboard.welcome', { PH: appTitle }),
  )
  const dashboardViews = $derived(
    sortDashboardViews(data.savedViews as DashboardView[], rawValue(session.settings, DASHBOARD_KEYS.VISIBLE_IDS), rawValue(session.settings, DASHBOARD_KEYS.SORT_ORDER)),
  )
  /** SettingsService.dashboardIsEmpty (set as soon as the views are loaded) */
  const dashboardIsEmpty = $derived(!data.savedViewsLoading && dashboardViews.length === 0)
  let tourCompleted = $state(false)
  const offerTour = $derived(dashboardIsEmpty && !tourCompleted && !truthy(rawValue(session.settings, DASHBOARD_KEYS.TOUR_COMPLETE)))
  const hint = $derived(tRich('dashboard.hint_saved_views_created'))

  /** Load the tour (ngx-ui-tour) only on start */
  const loadTour = () => import('../../lib/tour/tour.svelte').then((m) => m.tour)
  let tourLoaded: Awaited<ReturnType<typeof loadTour>> | null = null
  async function startTour() {
    tourLoaded = await loadTour()
    tourLoaded.start()
  }

  /** DashboardComponent.completeTour: end the running tour (calls completeTour), otherwise SettingsService.completeTour */
  function completeTour() {
    if (tourLoaded && tourLoaded.status !== 'off') return tourLoaded.end()
    if (!truthy(rawValue(session.settings, DASHBOARD_KEYS.TOUR_COMPLETE)) && session.can('change', 'uisettings')) tourCompleted = true
    void completeTourSetting()
  }

  /** DashboardComponent.onDrop → SettingsService.updateDashboardViewsSort */
  async function onDrop(previousIndex: number, currentIndex: number) {
    const views = [...dashboardViews]
    moveItemInArray(views, previousIndex, currentIndex)
    setSetting(DASHBOARD_KEYS.SORT_ORDER, [...new Set(views.map((v) => v.id))])
    try {
      await storeSettings()
      toastService.showSuccess(t('dashboard.dashboard_updated'))
    } catch (e) {
      toastService.showError(t('dashboard.error_updating_dashboard'), e)
    }
  }
</script>

<pngx-page-header tourAnchor="tour.dashboard"><PageHeader title={t('common.dashboard')} subTitle={subtitle} /></pngx-page-header>

<div class="row dashboard-grid g-4 pb-0">
  <div class="col-12 col-lg-8 col-xl-9 mb-4 dashboard-main">
    <div class="row row-cols-1 g-4 cdk-drop-list" class:cdk-drop-list-disabled={dropzone.active}
      use:cdkDropList={{ item: '.cdk-drag', handle: '.cdk-drag-handle', disabled: () => dropzone.active, dropped: onDrop }}>
      {#if data.savedViewsLoading}
        <div class="col">
          <div class="card shadow-sm bg-light">
            <div class="card-header">
              <div class="d-flex justify-content-between align-items-center">
                <div class="d-flex">
                  <div class="ms-n2 me-1">
                    <pngx-bs-icon name="grip-vertical">{@html icon('grip-vertical')}</pngx-bs-icon>
                  </div>
                  <h6 class="card-title mb-0"></h6>
                </div>
              </div>
            </div>
            <div class="card-body">&nbsp;</div>
          </div>
        </div>
      {/if}

      {#if offerTour}
        <div class="col">
          <pngx-welcome-widget><WelcomeWidget ondismiss={completeTour} onstarttour={startTour} /></pngx-welcome-widget>
        </div>
      {/if}

      {#if session.can('view', 'savedview')}
        {#if !offerTour && data.savedViews.length === 0}
          <div class="col">
            <div class="card shadow-sm bg-light opacity-50">
              <div class="card-body">
                <div class="text-center">
                  <p class="mb-0 fst-italic"><pngx-bs-icon name="info-circle" class="me-2">{@html icon('info-circle')}</pngx-bs-icon>{#each hint as s}{#if s.tag === 'START_LINK'}<a href={router.href('documents')}>{s.text}</a>{:else}{s.text}{/if}{/each}</p>
                </div>
              </div>
            </div>
          </div>
        {/if}
        {#each dashboardViews as v (v.id)}
          <div class="col">
            <pngx-saved-view-widget><SavedViewWidget savedView={v} /></pngx-saved-view-widget>
          </div>
        {/each}
      {/if}
    </div>
  </div>
  <div class="col-12 col-lg-4 col-xl-3 col-sidebar dashboard-aside">
    <div class="row row-cols-1 g-4 mb-4 sticky-lg-top z-0">
      <pngx-upload-file-widget><UploadFileWidget /></pngx-upload-file-widget>
      {#if session.can('view', 'uisettings')}
        <pngx-statistics-widget><StatisticsWidget /></pngx-statistics-widget>
      {/if}
    </div>
  </div>
</div>
