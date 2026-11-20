<!-- Port of common/input/color/color.component.html. Host <pngx-input-color> in the parent markup. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { newInputId } from '../input-id'
  import { Popover } from '../../../../lib/popover.svelte'
  import { randomColor } from '../../../../lib/matching'
  import ColorSlider from './ColorSlider.svelte'
  let {
    value = $bindable(),
    title = '',
    error = undefined,
    hint = undefined,
    onchange,
  }: { value?: string | null; title?: string; error?: string | string[]; hint?: string; onchange?: (v: string | null | undefined) => void } = $props()
  const inputId = newInputId()
  const colorPicker = new Popover({ placement: 'bottom', autoClose: 'outside' })
  function colorChanged(v: string) {
    value = v
    onchange?.(v)
  }
</script>

<div class="mb-3">
  {#if title}
    <label class="form-label" for={inputId}>{title}</label>
  {/if}

  <div class="input-group" class:is-invalid={error}>
    <button type="button" class="input-group-text" style:background-color={value} onclick={(e) => { e.stopPropagation(); colorPicker.toggle() }} aria-label={t('input.open_color_picker')}>&nbsp;&nbsp;&nbsp;</button>

    <input class="form-control" class:is-invalid={error} id={inputId} bind:value onchange={() => onchange?.(value)} use:colorPicker.triggerEl>
    {#if colorPicker.open}
      <ngb-popover-window role="tooltip" class="popover shadow" id={colorPicker.id} use:colorPicker.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body"><div style={"min-width: 200px;"} class="pb-3">
        <ColorSlider color={value ?? ''} onchange={colorChanged} />
      </div></div></ngb-popover-window>
    {/if}

    <button class="btn btn-outline-secondary" type="button" onclick={() => colorChanged(randomColor())} aria-label={t('input.choose_random_color')}>
      <pngx-bs-icon name="dice5">{@html icon('dice5')}</pngx-bs-icon>
    </button>

  </div>
  {#if hint}
    <small class="form-text text-muted">{hint}</small>
  {/if}
  <div class="invalid-feedback">
    {error}
  </div>
</div>
