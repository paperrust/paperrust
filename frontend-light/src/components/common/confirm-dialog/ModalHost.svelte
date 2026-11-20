<!-- Output of the open dialogs (NgbModal replica, lib/modal.svelte.ts) with the same DOM as ng-bootstrap. -->
<script lang="ts">
  import { modals } from '../../../lib/modal.svelte'

  function backdropClick(e: MouseEvent, ref: (typeof modals.stack)[number]) {
    if (e.target === e.currentTarget && !ref.staticBackdrop) ref.dismiss()
  }
  // NgbModalWindow._setFocus / _restoreFocus: [ngbAutofocus] or first focusable element (a disabled
  // close button does not count – so focus lands in the first input field in edit dialogs)
  const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [contenteditable], [tabindex]:not([tabindex="-1"])'
  function modalFocus(node: HTMLElement) {
    const elWithFocus = document.activeElement as HTMLElement | null
    if (!node.contains(document.activeElement)) {
      const auto = node.querySelector<HTMLElement>('[ngbAutofocus]')
      const first = Array.from(node.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => el.tabIndex !== -1)[0]
      ;(auto || first || node).focus()
    }
    return {
      destroy() {
        const target = elWithFocus && document.body.contains(elWithFocus) ? elWithFocus : document.body
        setTimeout(() => target.focus())
      },
    }
  }
  function keydown(e: KeyboardEvent) {
    const top = modals.open_[modals.open_.length - 1]
    if (top && e.key === 'Escape' && !top.staticBackdrop) top.dismiss()
  }
</script>

<svelte:window onkeydown={keydown} />

{#each modals.open_ as ref, i (ref.id)}
  {@const C = ref.component}
  <ngb-modal-backdrop style="z-index: {1055 + i * 10};" class="modal-backdrop fade show"></ngb-modal-backdrop>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_interactive_supports_focus -->
  <ngb-modal-window role="dialog" tabindex="-1" aria-modal="true" class="d-block modal fade show" style="z-index: {1055 + i * 10};" onclick={(e) => backdropClick(e, ref)} use:modalFocus><div role="document" class="modal-dialog {ref.size ? `modal-${ref.size}` : ''}"><div class="modal-content"><C {...ref.props} modal={ref} /></div></div></ngb-modal-window>
{/each}
