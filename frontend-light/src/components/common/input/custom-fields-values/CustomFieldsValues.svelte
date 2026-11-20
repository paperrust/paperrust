<!-- Port of common/input/custom-fields-values/custom-fields-values.component.html (values of assigned fields in
     workflows). Host <pngx-input-custom-fields-values> in the parent markup. Value: { [fieldId]: value }. -->
<script lang="ts">
  import '../../../../styles/components/input-custom-fields-values.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { listAllOf } from '../../../../api/admin'
  import InputText from '../text/InputText.svelte'
  import InputDate from '../date/InputDate.svelte'
  import InputNumber from '../number/InputNumber.svelte'
  import InputMonetary from '../monetary/InputMonetary.svelte'
  import InputCheck from '../check/InputCheck.svelte'
  import InputUrl from '../url/InputUrl.svelte'
  import InputDocumentLink from '../document-link/InputDocumentLink.svelte'
  import InputSelect from '../select/InputSelect.svelte'
  import InputTextarea from '../textarea/InputTextarea.svelte'
  import { valuesForSelected } from './custom-fields-values'

  let {
    value = $bindable(),
    selectedFields = [],
    onremoveselectedfield,
  }: { value?: Record<string, any> | null; selectedFields?: number[] | null; onremoveselectedfield?: (id: number) => void } = $props()

  let fields = $state<any[]>([])
  listAllOf<any>('custom_fields').then((r) => (fields = r.results))
  const getCustomField = (id: number) => fields.find((f) => f.id === id)

  // selectedFields setter: map the value onto the selected fields (missing = null)
  $effect(() => {
    const next = valuesForSelected(selectedFields ?? [], $state.snapshot(value) as any)
    if (JSON.stringify(next) !== JSON.stringify($state.snapshot(value))) value = next
  })
  const set = (id: number, v: any) => (value = { ...(value ?? {}), [id]: v })
</script>

<div class="list-group mt-3 selected-fields">
  {#each selectedFields ?? [] as fieldId (fieldId)}
    {@const f = getCustomField(fieldId)}
    <div class="list-group-item
      d-flex
      justify-content-between
      align-items-center">
      {#if f?.data_type === 'string'}
        <pngx-input-text class="flex-grow-1"><InputText value={value?.[fieldId]} onchange={(v) => set(fieldId, v)} title={f?.name} horizontal={true} /></pngx-input-text>
      {:else if f?.data_type === 'date'}
        <pngx-input-date class="flex-grow-1"><InputDate value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} /></pngx-input-date>
      {:else if f?.data_type === 'integer'}
        <pngx-input-number class="flex-grow-1"><InputNumber value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} showAdd={false} /></pngx-input-number>
      {:else if f?.data_type === 'float'}
        <pngx-input-number class="flex-grow-1"><InputNumber value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} showAdd={false} step={0.1} /></pngx-input-number>
      {:else if f?.data_type === 'monetary'}
        <pngx-input-monetary class="flex-grow-1"><InputMonetary value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} defaultCurrency={f?.extra_data?.default_currency} horizontal={true} /></pngx-input-monetary>
      {:else if f?.data_type === 'boolean'}
        <pngx-input-check class="flex-grow-1"><InputCheck value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} /></pngx-input-check>
      {:else if f?.data_type === 'url'}
        <pngx-input-url class="flex-grow-1"><InputUrl value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} /></pngx-input-url>
      {:else if f?.data_type === 'documentlink'}
        <pngx-input-document-link class="flex-grow-1"><InputDocumentLink value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} horizontal={true} /></pngx-input-document-link>
      {:else if f?.data_type === 'select'}
        <pngx-input-select class="flex-grow-1"><InputSelect value={value?.[fieldId]} onchange={(v) => set(fieldId, v)} title={f?.name} items={f?.extra_data.select_options} bindLabel="label" allowNull={true} horizontal={true} /></pngx-input-select>
      {:else if f?.data_type === 'longtext'}
        <pngx-input-textarea class="flex-grow-1"><InputTextarea value={value?.[fieldId]} onchange={(v: any) => set(fieldId, v)} title={f?.name} /></pngx-input-textarea>
      {/if}
      <button type="button" class="btn btn-link text-danger" onclick={() => onremoveselectedfield?.(fieldId)} aria-label={t('input.remove_custom_field')}>
        <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon>
      </button>
    </div>
  {/each}
</div>
