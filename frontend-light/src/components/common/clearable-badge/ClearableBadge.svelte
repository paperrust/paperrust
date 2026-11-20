<!-- Port of common/clearable-badge. Host <pngx-clearable-badge> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/clearable-badge.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  let { number = undefined, selected = undefined, oncleared }: { number?: number; selected?: boolean; oncleared?: () => void } = $props()
  const isNumbered = $derived(number !== undefined && number > -1)
  const active = $derived(!!selected || isNumbered)
  function onClick(e: MouseEvent) {
    oncleared?.()
    e.stopImmediatePropagation()
    e.preventDefault()
  }
</script>

{#if active}
  <button class="position-absolute top-0 start-100 translate-middle badge bg-secondary border border-light rounded-pill p-1" title={t('common.clear')} onclick={onClick}>
    {#if !isNumbered && selected}
      <pngx-bs-icon class="check" width="1em" height="1em" name="check-lg">{@html icon('check-lg', '1em', '1em')}</pngx-bs-icon>
    {/if}
    {#if isNumbered}
      <div class="number">{number}<span class="visually-hidden">selected</span></div>
    {/if}
    <pngx-bs-icon class="x" width=".9em" height="1em" name="x-lg">{@html icon('x-lg', '.9em', '1em')}</pngx-bs-icon>
  </button>
{/if}
