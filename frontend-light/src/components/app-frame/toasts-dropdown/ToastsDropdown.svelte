<!-- Port of app-frame/toasts-dropdown/toasts-dropdown.component.html (notifications).
     Host <pngx-toasts-dropdown> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/toasts-dropdown.css'
  import { onDestroy } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { toastService, type Toast as T } from '../../../stores/toasts.svelte'
  import { LazyToast } from '../../common/toast/lazy.svelte'

  /** drives the relative times, only ticks while the menu is open */
  let now = $state(new Date())
  /** unread notifications when opening */
  let newIds = $state(new Set<string>())
  let clock: ReturnType<typeof setInterval> | undefined

  const toasts = $derived(toastService.toasts)
  const unread = $derived(toasts.filter((x) => !x.read))
  const hasUnreadError = $derived(unread.some((x) => x.level === 'error'))

  const dd = new Dropdown({ static: true, onOpenChange })
  const Toast = $derived(LazyToast.get(dd.open && toasts.length > 0))

  function onOpenChange(open: boolean) {
    toastService.suppressPopupToasts = open
    clearInterval(clock)
    newIds = new Set(open ? unread.map((x) => x.id!) : [])
    if (open) {
      toastService.markAllRead()
      now = new Date()
      clock = setInterval(() => (now = new Date()), 30000)
    }
  }
  onDestroy(() => clearInterval(clock))

  /** Action (undo, open document …) removes the message and closes the menu */
  function onActioned(toast: T) {
    toastService.closeToast(toast)
    dd.close()
  }
</script>

<li class="nav-item position-relative {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
  {#if unread.length}
    <span class="notification-count badge rounded-pill z-3 pe-none position-absolute" class:bg-danger={hasUnreadError} class:bg-secondary={!hasUnreadError}>{unread.length}</span>
  {/if}
  <button class="dropdown-toggle btn navbar-action border-0" id="notificationsDropdown" aria-label={t('common.notifications')} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon width="1.3em" height="1.3em" name="bell">{@html icon('bell', '1.3em', '1.3em')}</pngx-bs-icon>
  </button>
  {#if dd.open}
    <div class="dropdown-menu dropdown-menu-end shadow p-2 show" aria-labelledby="notificationsDropdown" use:dd.menuEl>
      <div class="d-flex align-items-center px-1 mb-2">
        <h6 class="mb-0">{t('common.notifications')}</h6>
        {#if toasts.length}
          <button class="btn btn-link btn-sm p-0 ms-auto text-body-secondary text-decoration-none clear-all"
            onclick={() => toastService.clearToasts()}>
            <pngx-bs-icon width="1em" height="1em" name="check2-all" class="me-1">{@html icon('check2-all', '1em', '1em')}</pngx-bs-icon>{t('frame.clear_all')}
          </button>
        {/if}
      </div>
      {#if toasts.length === 0}
        <div class="text-center text-body-secondary small py-4">
          <pngx-bs-icon width="1.5em" height="1.5em" name="bell" class="d-block mx-auto mb-2 opacity-50">{@html icon('bell', '1.5em', '1.5em')}</pngx-bs-icon>{t('common.no_notifications')}
        </div>
      {/if}
      <div class="scroll-list d-flex flex-column gap-2">
        {#if Toast}
        {#each toasts as toast (toast.id)}
          <pngx-toast><Toast autohide={false} {toast} {now} isNew={newIds.has(toast.id!)} onclosed={(x) => toastService.closeToast(x)} onactioned={onActioned} /></pngx-toast>
        {/each}
        {/if}
      </div>
    </div>
  {/if}
</li>
