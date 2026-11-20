<!-- Port of common/edit-dialog/custom-field-edit-dialog/custom-field-edit-dialog.component.html
     (selection options in pages of 8, like SELECT_OPTION_PAGE_SIZE). -->
<script lang="ts">
  import { tick } from 'svelte'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import type { CustomField } from '../../../../api/v1'
  import { dataTypeLabels } from '../../../../lib/matching'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import Pagination from '../../pagination/Pagination.svelte'

  const SELECT_OPTION_PAGE_SIZE = 8
  let props: EditDialogProps<CustomField> = $props()
  const dlg = new EditDialogState<CustomField>('custom_fields', props, {
    name: null,
    data_type: null,
    extra_data: { select_options: [], default_currency: null },
  })
  // extra_data always complete (patchValue only takes over known subfields)
  dlg.form.extra_data = {
    select_options: [],
    default_currency: (props.object?.extra_data as any)?.default_currency ?? null,
  }
  const typeFieldDisabled = dlg.mode === 'edit'
  let allSelectOptions = $state<{ label: string | null; id: string | null }[]>(
    props.object?.data_type === 'select' ? [...((props.object.extra_data?.select_options as any[]) ?? [])].map((o) => ({ ...o })) : [],
  )
  let selectOptionsPage = $state(1)
  const pageOptions = $derived(allSelectOptions.slice((selectOptionsPage - 1) * SELECT_OPTION_PAGE_SIZE, selectOptionsPage * SELECT_OPTION_PAGE_SIZE))
  let optionInputs: HTMLInputElement[] = $state([])
  const title = $derived(dlg.mode === 'create' ? t('dialog.custom_field.create_new_custom_field') : t('dialog.custom_field.edit_custom_field'))

  async function addSelectOption() {
    allSelectOptions.push({ label: null, id: null })
    selectOptionsPage = Math.ceil(allSelectOptions.length / SELECT_OPTION_PAGE_SIZE)
    await tick()
    optionInputs.filter(Boolean).at(-1)?.focus()
  }
  function removeSelectOption(index: number) {
    allSelectOptions.splice(index + (selectOptionsPage - 1) * SELECT_OPTION_PAGE_SIZE, 1)
    const totalPages = Math.max(1, Math.ceil(allSelectOptions.length / SELECT_OPTION_PAGE_SIZE))
    selectOptionsPage = Math.min(selectOptionsPage, totalPages)
  }
  function save() {
    dlg.save(() => {
      const v: any = $state.snapshot(dlg.form)
      // disabled field (data type when editing) is not part of the form value
      if (typeFieldDisabled) delete v.data_type
      if (dlg.form.data_type === 'select') v.extra_data.select_options = $state.snapshot(allSelectOptions)
      return v
    })
  }
</script>

<pngx-custom-field-edit-dialog><form onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    {#if dlg.object?.id}
      <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {dlg.object.id}</span>
    {/if}
    <button type="button" disabled={!dlg.closeEnabled} class="btn-close" aria-label="Close" onclick={() => dlg.cancel()}>
    </button>
  </div>
  <div class="modal-body">
    <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
    <pngx-input-select><InputSelect title={t('common.data_type')} items={dataTypeLabels()} bind:value={dlg.form.data_type} disabled={typeFieldDisabled} /></pngx-input-select>
    {#if typeFieldDisabled}
      <small class="d-block mt-n2">{t('dialog.custom_field.data_type_cannot_changed')}</small>
    {/if}
    <div>
      {#if dlg.form.data_type === 'select'}
        <button type="button" class="btn btn-sm btn-primary my-2" onclick={addSelectOption}>
          <span>{t('dialog.custom_field.add_option')}</span><pngx-bs-icon class="ms-1" name="plus-circle">{@html icon('plus-circle')}</pngx-bs-icon>
        </button>
        <div>
          {#each pageOptions as option, i (option)}
            <div class="input-group input-group-sm my-2">
              <input bind:this={optionInputs[i]} type="text" class="form-control" bind:value={option.label} autocomplete="off">
              <input type="hidden" value={option.id}>
              <button type="button" class="btn btn-outline-danger" onclick={() => removeSelectOption(i)}>{t('common.delete')}</button>
            </div>
          {/each}
        </div>
        {#if allSelectOptions.length > SELECT_OPTION_PAGE_SIZE}
          <ngb-pagination class="d-flex justify-content-end" role="navigation" size="sm"><Pagination pageSize={SELECT_OPTION_PAGE_SIZE} collectionSize={allSelectOptions.length} page={selectOptionsPage} maxSize={5} onpage={(p) => (selectOptionsPage = p)} /></ngb-pagination>
        {/if}
      {:else if dlg.form.data_type === 'monetary'}
        <div class="my-3">
          <pngx-input-text><InputText title={t('dialog.custom_field.default_currency')} hint={t('dialog.custom_field.3_character_currency_code')} bind:value={dlg.form.extra_data.default_currency} placeholder={t('dialog.custom_field.use_locale')} autocomplete="off" /></pngx-input-text>
        </div>
      {/if}
    </div>
  </div>
  <div class="modal-footer">
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-custom-field-edit-dialog>
