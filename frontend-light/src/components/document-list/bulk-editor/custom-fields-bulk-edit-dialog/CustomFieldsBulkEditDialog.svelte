<!-- Port of document-list/bulk-editor/custom-fields-bulk-edit-dialog: set values for selected fields
     (bulk_edit modify_custom_fields with add_custom_fields = {field ID: value}). Success/failure via onsucceeded/onfailed. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { api } from '../../../../api/client'
  import type { CustomField, DocumentSelectionQuery } from '../../../../api/v1'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import InputSelect from '../../../common/input/select/InputSelect.svelte'
  import InputText from '../../../common/input/text/InputText.svelte'
  import InputDate from '../../../common/input/date/InputDate.svelte'
  import InputNumber from '../../../common/input/number/InputNumber.svelte'
  import InputMonetary from '../../../common/input/monetary/InputMonetary.svelte'
  import InputCheck from '../../../common/input/check/InputCheck.svelte'
  import InputUrl from '../../../common/input/url/InputUrl.svelte'
  import InputDocumentLink from '../../../common/input/document-link/InputDocumentLink.svelte'
  import InputTextarea from '../../../common/input/textarea/InputTextarea.svelte'
  import { formValue, keepValues } from './custom-fields-bulk-edit'

  let {
    modal,
    customFields = [],
    fieldsToAddIds: initialAdd = [],
    fieldsToRemoveIds = [],
    selection = { documents: [] },
    selectionCount = undefined,
    onsucceeded,
    onfailed,
  }: {
    modal: ModalRef<any>
    customFields?: CustomField[]
    fieldsToAddIds?: number[]
    fieldsToRemoveIds?: number[]
    selection?: DocumentSelectionQuery
    selectionCount?: number
    onsucceeded?: () => void
    onfailed?: (e: unknown) => void
  } = $props()

  let networkActive = $state(false)
  // svelte-ignore state_referenced_locally
  let fieldsToAddIds = $state<number[]>(initialAdd.concat())
  // FormGroup: one control per field ID, initial value null
  // svelte-ignore state_referenced_locally
  let values = $state<Record<string, any>>(keepValues({}, initialAdd))
  const fieldsToAdd = $derived(customFields.filter((f) => fieldsToAddIds.includes(f.id)))
  const documentCount = $derived(selectionCount ?? selection.documents?.length ?? 0)

  function setFieldsToAdd(ids: number[] | null | undefined) {
    fieldsToAddIds = ids ?? []
    values = keepValues(values, fieldsToAddIds)
  }
  function removeField(fieldId: number) {
    setFieldsToAdd(fieldsToAddIds.filter((id) => id !== fieldId))
  }
  async function save(e?: Event) {
    e?.preventDefault()
    networkActive = true
    try {
      await api.bulkEdit(selection, 'modify_custom_fields', {
        add_custom_fields: formValue(values, fieldsToAddIds),
        remove_custom_fields: fieldsToRemoveIds,
      })
      modal.close()
      onsucceeded?.()
    } catch (err) {
      onfailed?.(err)
    } finally {
      networkActive = false
    }
  }
</script>

<pngx-custom-fields-bulk-edit-dialog><form onsubmit={save} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('dialog.custom_fields_bulk.title', { VAR_PLURAL: documentCount, INTERPOLATION: documentCount })}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}>
    </button>
  </div>
  <div class="modal-body">
    <pngx-input-select><InputSelect title="" multiple={true} items={customFields} value={fieldsToAddIds} onchange={setFieldsToAdd}
      placeholder={t('dialog.custom_fields_bulk.select_fields')} /></pngx-input-select>
    <div class="d-flex flex-column gap-2">
      {#each fieldsToAdd as field (field.id)}
        <div class="d-flex gap-2">
          {#if field.data_type === 'string'}
            <pngx-input-text class="w-100"><InputText bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-text>
          {:else if field.data_type === 'date'}
            <pngx-input-date class="w-100"><InputDate bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-date>
          {:else if field.data_type === 'integer'}
            <pngx-input-number class="w-100"><InputNumber bind:value={values[field.id]} title={field.name} showAdd={false} horizontal={true} /></pngx-input-number>
          {:else if field.data_type === 'float'}
            <pngx-input-number class="w-100"><InputNumber bind:value={values[field.id]} title={field.name} showAdd={false} step={0.1} horizontal={true} /></pngx-input-number>
          {:else if field.data_type === 'monetary'}
            <pngx-input-monetary class="w-100"><InputMonetary bind:value={values[field.id]} title={field.name} defaultCurrency={field.extra_data?.default_currency} horizontal={true} /></pngx-input-monetary>
          {:else if field.data_type === 'boolean'}
            <pngx-input-check class="w-100"><InputCheck bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-check>
          {:else if field.data_type === 'url'}
            <pngx-input-url class="w-100"><InputUrl bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-url>
          {:else if field.data_type === 'documentlink'}
            <pngx-input-document-link class="w-100"><InputDocumentLink bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-document-link>
          {:else if field.data_type === 'select'}
            <pngx-input-select class="w-100"><InputSelect bind:value={values[field.id]} title={field.name} items={field.extra_data?.select_options} bindLabel="label" allowNull={true} horizontal={true} /></pngx-input-select>
          {:else if field.data_type === 'longtext'}
            <pngx-input-textarea class="w-100"><InputTextarea bind:value={values[field.id]} title={field.name} horizontal={true} /></pngx-input-textarea>
          {/if}
          <button type="button" class="btn btn-outline-danger mb-3" onclick={() => removeField(field.id)} aria-label={t('input.remove_custom_field')}>
            <pngx-bs-icon name="x">{@html icon('x')}</pngx-bs-icon>
          </button>
        </div>
      {/each}
    </div>
  </div>
  <div class="modal-footer">
    {#if fieldsToRemoveIds.length}
      <p class="mb-0 small"><em>{t('dialog.custom_fields_bulk.also_removes', { VAR_PLURAL: fieldsToRemoveIds.length, INTERPOLATION: fieldsToRemoveIds.length })}</em></p>
    {/if}
    <button type="button" class="btn btn-outline-secondary" onclick={() => modal.close()}
      disabled={networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary"
      disabled={networkActive || fieldsToRemoveIds.length + fieldsToAddIds.length === 0}>{t('common.save')}</button>
  </div>
</form></pngx-custom-fields-bulk-edit-dialog>
