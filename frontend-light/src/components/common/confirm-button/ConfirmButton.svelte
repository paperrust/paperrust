<!-- Port of common/confirm-button/confirm-button.component.html (button + ngbPopover "Sure? Yes").
     Host <pngx-confirm-button> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/confirm-button.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Popover } from '../../../lib/popover.svelte'

  let {
    label = '',
    confirmMessage = t('ui.sure'),
    buttonClasses = 'btn-primary',
    iconName = undefined,
    disabled = false,
    onconfirm,
  }: { label?: string; confirmMessage?: string; buttonClasses?: string; iconName?: string; disabled?: boolean; onconfirm?: () => void } = $props()

  // Open only via onClick (like popover.open()); the ngbPopover default trigger 'click' is intercepted
  const popover = new Popover({ autoClose: true, triggers: 'manual' })
  let confirming = false
  $effect(() => {
    if (!popover.open) confirming = false
  })
  function onClick(e: MouseEvent) {
    if (!confirming) {
      confirming = true
      popover.show()
    }
    e.preventDefault()
    e.stopImmediatePropagation()
  }
  function onConfirm(e: MouseEvent) {
    onconfirm?.()
    confirming = false
    popover.hide()
    e.preventDefault()
    e.stopImmediatePropagation()
  }
</script>

<button type="button" class="btn {buttonClasses}" onclick={onClick} {disabled} use:popover.triggerEl>
  {#if iconName}
    <pngx-bs-icon class:me-1={label} name={iconName}>{@html icon(iconName)}</pngx-bs-icon>
  {/if}{label}</button>
{#if popover.open}
  <ngb-popover-window role="tooltip" class="popover popover-slim" id={popover.id} use:popover.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
    <div>
      {confirmMessage}&nbsp;<button class="btn btn-link btn-sm text-danger p-0 m-0 lh-1" type="button" onclick={onConfirm}>{t('common.yes')}</button>
    </div>
  </div></ngb-popover-window>
{/if}
