<!-- Values of the assigned custom fields (assign_custom_fields_values, { fieldId: value }) – input per field type. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { data } from '../../../stores/data.svelte'
  let { fields, values, idPrefix, onchange }: { fields: number[]; values: Record<string, any>; idPrefix: string; onchange: (v: Record<string, any>) => void } = $props()
  const set = (id: number, v: unknown) => onchange({ ...values, [id]: v === '' ? null : v })
  const docIds = (s: string) =>
    s
      .split(/[\s,;]+/)
      .map((x) => Number(x))
      .filter((n) => Number.isInteger(n) && n > 0)
</script>

{#if fields.length}
  <div class="au-group" role="group" aria-label={t('m.auto.field_values')}>
    <h3>{t('m.auto.field_values')}</h3>
    <p class="au-hint">{t('m.auto.field_values_hint')}</p>
    {#each fields as fid (fid)}
      {@const f = data.customFields.find((x) => x.id === fid)}
      {@const id = `${idPrefix}-cf-${fid}`}
      <div class="au-field">
        <label for={id}>{f?.name ?? `#${fid}`}</label>
        {#if f?.data_type === 'boolean'}
          <select {id} class="d2-select" value={values[fid] === true ? 'true' : values[fid] === false ? 'false' : ''} onchange={(e) => set(fid, e.currentTarget.value === '' ? null : e.currentTarget.value === 'true')}>
            <option value="">{t('m.auto.cf_empty')}</option>
            <option value="true">{t('common.yes')}</option>
            <option value="false">{t('common.no')}</option>
          </select>
        {:else if f?.data_type === 'select'}
          <select {id} class="d2-select" value={values[fid] ?? ''} onchange={(e) => set(fid, e.currentTarget.value)}>
            <option value="">{t('m.auto.cf_empty')}</option>
            {#each f.extra_data?.select_options ?? [] as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
          </select>
        {:else if f?.data_type === 'date'}
          <input {id} class="d2-input" type="date" value={values[fid] ?? ''} oninput={(e) => set(fid, e.currentTarget.value)} />
        {:else if f?.data_type === 'integer'}
          <input {id} class="d2-input" type="number" step="1" value={values[fid] ?? ''} oninput={(e) => set(fid, e.currentTarget.value === '' ? null : Number(e.currentTarget.value))} />
        {:else if f?.data_type === 'float'}
          <input {id} class="d2-input" type="number" step="any" value={values[fid] ?? ''} oninput={(e) => set(fid, e.currentTarget.value === '' ? null : Number(e.currentTarget.value))} />
        {:else if f?.data_type === 'longtext'}
          <textarea {id} class="d2-textarea" rows="3" value={values[fid] ?? ''} oninput={(e) => set(fid, e.currentTarget.value)}></textarea>
        {:else if f?.data_type === 'documentlink'}
          <input {id} class="d2-input" inputmode="numeric" value={Array.isArray(values[fid]) ? values[fid].join(', ') : ''} oninput={(e) => set(fid, docIds(e.currentTarget.value))} />
          <p class="au-hint">{t('m.auto.cf_doclink_hint')}</p>
        {:else if f?.data_type === 'monetary'}
          <input {id} class="d2-input" value={values[fid] ?? ''} placeholder={`${f.extra_data?.default_currency ?? 'EUR'}12.50`} oninput={(e) => set(fid, e.currentTarget.value)} />
          <p class="au-hint">{t('m.auto.cf_monetary_hint')}</p>
        {:else}
          <input {id} class="d2-input" type={f?.data_type === 'url' ? 'url' : 'text'} value={values[fid] ?? ''} oninput={(e) => set(fid, e.currentTarget.value)} />
        {/if}
      </div>
    {/each}
  </div>
{/if}
