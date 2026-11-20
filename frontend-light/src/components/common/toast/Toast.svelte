<!-- Port of common/toast/toast.component.html (+ ngb-toast/ngb-progressbar markup from ng-bootstrap).
     Host <pngx-toast> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/toast.css'
  import '../../../styles/parts/toast.css'
  import { onDestroy } from 'svelte'
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import type { Toast } from '../../../stores/toasts.svelte'
  import { relativeDate } from '../../document-detail/document-history/history'

  let {
    toast,
    autohide = true,
    now = undefined,
    isNew = false,
    onhidden,
    onclosed,
    onactioned,
  }: {
    toast: Toast
    autohide?: boolean
    /** every new value re-renders the relative time */
    now?: Date
    isNew?: boolean
    onhidden?: (t: Toast) => void
    onclosed?: (t: Toast) => void
    onactioned?: (t: Toast) => void
  } = $props()

  const REFRESH = 150
  // svelte-ignore state_referenced_locally
  let delayRemaining = $state<number>(autohide ? toast.delay : 0)
  let shown = $state(true)
  let copied = $state(false)
  let hovered = false
  let focused = false
  let timer: ReturnType<typeof setInterval> | undefined

  // ToastComponent.onShown: countdown (paused on mouse/focus), then fade out
  if (autohide) {
    timer = setInterval(() => {
      if (hovered || focused) return
      delayRemaining = Math.max(0, delayRemaining - REFRESH)
      if (delayRemaining === 0) {
        clearInterval(timer)
        hide()
      }
    }, REFRESH)
  }
  onDestroy(() => clearInterval(timer))

  /** NgbToast.hide: fade-out transition, then (hidden) */
  function hide() {
    shown = false
    setTimeout(() => onhidden?.(toast), 150)
  }

  const relativeTimestamp = $derived.by(() => {
    void now
    if (!toast.timestamp) return ''
    const r = relativeDate(toast.timestamp)
    return t(r.key).replace('%s', String(r.n))
  })
  const percent = $derived(autohide ? (100 * Math.min(Math.max(delayRemaining, 0), toast.delay)) / toast.delay : 0)
  const seconds = $derived(new Intl.NumberFormat(locale(), { maximumFractionDigits: 0 }).format(delayRemaining / 1000))

  function isDetailedError(error: any): boolean {
    return typeof error === 'object' && 'status' in error && 'statusText' in error && 'url' in error && 'message' in error && 'error' in error
  }
  function copyError(error: any) {
    navigator.clipboard?.writeText(JSON.stringify(error))
    copied = true
    setTimeout(() => (copied = false), 3000)
  }
  function getErrorText(error: any) {
    let text: any = error.error?.detail ?? error.error ?? ''
    if (typeof text === 'object') text = JSON.stringify(text)
    return `${text.slice(0, 200)}${text.length > 200 ? '...' : ''}`
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<ngb-toast class="toast fade" class:show={shown} class:mb-2={autohide} class:toast-list-item={!autohide} class:toast-new={isNew}
  class:toast-success={toast.level === 'success'} class:toast-error={toast.level === 'error'}
  role="alert" aria-live="polite" aria-atomic="true"
  onmouseenter={() => (hovered = true)} onmouseleave={() => (hovered = false)}
  onfocusin={() => (focused = true)} onfocusout={() => (focused = false)}>
  <div class="toast-body">
        {#if autohide}
            <ngb-progressbar class="position-absolute w-100 start-0 bottom-0 pe-none progress" role="progressbar" aria-valuenow={Math.min(Math.max(delayRemaining, 0), toast.delay)} aria-valuemin="0" aria-valuemax={toast.delay} aria-label="progress bar"><div class="progress-bar" style:width="{percent}%"></div></ngb-progressbar>
            <span class="visually-hidden">{seconds} seconds</span>
        {/if}
        <div class="d-flex align-items-top">
        {#if toast.level === 'success'}
                <pngx-bs-icon width="0.9em" height="0.9em" name="check-circle" class="toast-icon">{@html icon('check-circle', '0.9em', '0.9em')}</pngx-bs-icon>
        {:else if toast.level === 'error'}
                <pngx-bs-icon width="0.9em" height="0.9em" name="exclamation-triangle" class="toast-icon">{@html icon('exclamation-triangle', '0.9em', '0.9em')}</pngx-bs-icon>
        {:else}
                <pngx-bs-icon width="0.9em" height="0.9em" name="info-circle" class="toast-icon">{@html icon('info-circle', '0.9em', '0.9em')}</pngx-bs-icon>
        {/if}
        <div class="toast-content">
            <p class="ms-2 mb-0 text-break">{toast.content}</p>
            {#if toast.error}
            <details class="ms-2">
                <div class="mt-2 ms-n4 me-n2 small">
                {#if isDetailedError(toast.error)}
                    <dl class="row mb-0">
                    <dt class="col-sm-3 fw-normal text-end">URL</dt>
                    <dd class="col-sm-9">{toast.error.url}</dd>
                    <dt class="col-sm-3 fw-normal text-end">{t('common.status')}</dt>
                    <dd class="col-sm-9">{toast.error.status} <em>{toast.error.statusText}</em></dd>
                    <dt class="col-sm-3 fw-normal text-end">{t('common.error')}</dt>
                    <dd class="col-sm-9">{getErrorText(toast.error)}</dd>
                    </dl>
                {/if}
                <div class="row">
                    <div class="col offset-sm-3">
                    <button class="btn btn-sm btn-outline-secondary" onclick={() => copyError(toast.error)}>
                        {#if !copied}
                        <pngx-bs-icon name="clipboard" class="me-1">{@html icon('clipboard')}</pngx-bs-icon>
                        {/if}
                        {#if copied}
                        <pngx-bs-icon name="clipboard-check" class="me-1">{@html icon('clipboard-check')}</pngx-bs-icon>
                        {/if}{t('ui.copy_raw_error')}
                    </button>
                    </div>
                </div>
                </div>
            </details>
            {/if}
            {#if toast.action}
            <button class="btn btn-sm toast-action ms-2 mt-2" onclick={() => { onactioned?.(toast); toast.action?.() }}>{toast.actionName}</button>
            {/if}
        </div>
        {#if !autohide && toast.timestamp}
            <span class="toast-time small text-body-secondary text-nowrap ms-auto ps-2">{relativeTimestamp}</span>
        {/if}
        <button type="button" class="btn-close ms-auto flex-shrink-0" data-bs-dismiss="toast" aria-label="Close" onclick={() => onclosed?.(toast)}></button>
    </div>
  </div>
</ngb-toast>
