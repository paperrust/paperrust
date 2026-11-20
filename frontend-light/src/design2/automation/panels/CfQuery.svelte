<!-- Condition for custom fields (filter_custom_field_query): simple builder (one level "all"/"any"
     of several field conditions). Queries the builder cannot represent (nested) remain editable as
     text – so no option is lost. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { data } from '../../../stores/data.svelte'
  import { CUSTOM_FIELD_QUERY_OPERATOR_LABELS } from '../../../lib/custom-field-query'
  import Icon from '../../components/Icon.svelte'
  import { cfOperators, parseCfQuery, serializeCfQuery, type CfAtom, type CfSimple } from '../model'

  let { value, id, onchange }: { value: string | null; id: string; onchange: (v: string | null) => void } = $props()
  // svelte-ignore state_referenced_locally
  let simple = $state<CfSimple | null>(parseCfQuery(value))
  // svelte-ignore state_referenced_locally
  let raw = $state(value ?? '')
  let rawError = $state(false)

  const typeOf = (fid: number) => data.customFields.find((f) => f.id === fid)?.data_type ?? 'string'
  const opLabel = (op: string) => t((CUSTOM_FIELD_QUERY_OPERATOR_LABELS as Record<string, string>)[op] ?? op)
  function emit() {
    if (simple) onchange(serializeCfQuery(simple))
  }
  function add() {
    const f = data.customFields[0]
    simple?.atoms.push([f?.id ?? 0, cfOperators(f?.data_type ?? 'string')[0], ''])
    emit()
  }
  function setField(a: CfAtom, fid: number) {
    a[0] = fid
    const ops = cfOperators(typeOf(fid))
    if (!ops.includes(a[1])) a[1] = ops[0]
    a[2] = a[1] === 'exists' || a[1] === 'isnull' ? true : ''
    emit()
  }
  function setOp(a: CfAtom, op: string) {
    a[1] = op
    if (op === 'exists' || op === 'isnull') a[2] = true
    else if (typeof a[2] === 'boolean' && typeOf(a[0]) !== 'boolean') a[2] = ''
    emit()
  }
  function setValue(a: CfAtom, v: string) {
    const ty = typeOf(a[0])
    if (a[1] === 'in')
      a[2] = v
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean)
    else if (a[1] === 'contains')
      a[2] = v
        .split(/[\s,;]+/)
        .map(Number)
        .filter((n) => n > 0)
    else if ((ty === 'integer' || ty === 'float') && v !== '' && !Number.isNaN(Number(v))) a[2] = Number(v)
    else if (ty === 'boolean') a[2] = v === 'true'
    else a[2] = v
    emit()
  }
  function rawInput(v: string) {
    raw = v
    if (!v.trim()) {
      rawError = false
      onchange(null)
      return
    }
    try {
      JSON.parse(v)
      rawError = false
      onchange(v)
    } catch {
      rawError = true
    }
  }
  const showValue = (a: CfAtom) => a[1] !== 'exists' && a[1] !== 'isnull'
  const valueText = (a: CfAtom) => (Array.isArray(a[2]) ? a[2].join(', ') : a[2] == null ? '' : String(a[2]))
</script>

<div class="au-field" role="group" aria-labelledby="{id}-l">
  <span class="lab" id="{id}-l">{t('m.auto.cond.custom_field_query')}</span>
  {#if simple}
    {#if simple.atoms.length > 1}
      <div class="au-seg" role="group" aria-label={t('m.auto.cf_join')}>
        <button type="button" aria-pressed={simple.op === 'AND'} onclick={() => simple && ((simple.op = 'AND'), emit())}>{t('m.auto.cf_all')}</button>
        <button type="button" aria-pressed={simple.op === 'OR'} onclick={() => simple && ((simple.op = 'OR'), emit())}>{t('m.auto.cf_any')}</button>
      </div>
    {/if}
    {#each simple.atoms as a, i (i)}
      <div class="au-cf-atom">
        <select class="d2-select" aria-label={t('m.auto.cf_field')} value={a[0]} onchange={(e) => setField(a, Number(e.currentTarget.value))}>
          {#each data.customFields as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
        </select>
        <select class="d2-select" aria-label={t('m.auto.cf_operator')} value={a[1]} onchange={(e) => setOp(a, e.currentTarget.value)}>
          {#each cfOperators(typeOf(a[0])) as op (op)}<option value={op}>{opLabel(op)}</option>{/each}
        </select>
        {#if showValue(a)}
          {#if typeOf(a[0]) === 'boolean'}
            <select class="d2-select" aria-label={t('m.auto.cf_value')} value={String(a[2])} onchange={(e) => setValue(a, e.currentTarget.value)}>
              <option value="true">{t('common.yes')}</option><option value="false">{t('common.no')}</option>
            </select>
          {:else if typeOf(a[0]) === 'select' && a[1] === 'exact'}
            {@const f = data.customFields.find((x) => x.id === a[0])}
            <select class="d2-select" aria-label={t('m.auto.cf_value')} value={String(a[2] ?? '')} onchange={(e) => setValue(a, e.currentTarget.value)}>
              <option value=""></option>
              {#each f?.extra_data?.select_options ?? [] as o (o.id)}<option value={o.id}>{o.label}</option>{/each}
            </select>
          {:else}
            <input class="d2-input" aria-label={t('m.auto.cf_value')} type={typeOf(a[0]) === 'date' ? 'date' : 'text'} value={valueText(a)} oninput={(e) => setValue(a, e.currentTarget.value)} />
          {/if}
        {:else}
          <span></span>
        {/if}
        <button type="button" class="d2-btn ghost icon" aria-label={t('m.auto.remove')} onclick={() => (simple?.atoms.splice(i, 1), emit())}><Icon name="x-lg" /></button>
      </div>
    {/each}
    {#if data.customFields.length}
      <div><button type="button" class="d2-btn sm" id="{id}-add" onclick={add}><Icon name="plus-lg" />{t('m.auto.cf_add')}</button></div>
    {:else}
      <p class="au-hint">{t('m.auto.cf_no_fields')}</p>
    {/if}
  {:else}
    <p class="au-hint">{t('m.auto.cf_complex')}</p>
    <textarea class="d2-textarea" rows="3" aria-label={t('m.auto.cond.custom_field_query')} value={raw} oninput={(e) => rawInput(e.currentTarget.value)}></textarea>
    {#if rawError}<p class="au-err">{t('m.auto.cf_invalid')}</p>{/if}
  {/if}
</div>
