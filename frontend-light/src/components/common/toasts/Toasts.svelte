<!-- Port of common/toasts/toasts.component.html (popups, at most MAX_VISIBLE_TOASTS).
     Host <pngx-toasts> in the parent markup (App.svelte). -->
<script lang="ts">
  import '../../../styles/components/toasts.css'
  import { toastService, type Toast as T } from '../../../stores/toasts.svelte'
  import { LazyToast } from '../toast/lazy.svelte'

  const Toast = $derived(LazyToast.get(toastService.popups.length > 0))

  function closeToast(toast: T) {
    toastService.closeToast(toast)
    toastService.removePopup(toast)
  }
</script>

{#if Toast}
{#each toastService.popups as toast (toast.id)}
  <pngx-toast><Toast {toast} autohide={true} onclosed={(t) => toastService.removePopup(t)} onactioned={closeToast} onhidden={(t) => toastService.removePopup(t)} /></pngx-toast>
{/each}
{/if}
