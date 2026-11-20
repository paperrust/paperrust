<!-- Key/value list (webhook fields and headers). Value: object; empty keys are not saved. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import Icon from '../../components/Icon.svelte'
  let { value, label, id, onchange }: { value: Record<string, string> | null | undefined; label: string; id: string; onchange: (v: Record<string, string>) => void } = $props()
  // svelte-ignore state_referenced_locally
  let rows = $state<[string, string][]>(Object.entries(value ?? {}))
  const emit = () => onchange(Object.fromEntries(rows.filter(([k]) => k.trim().length)))
</script>

<div class="au-field">
  <span class="lab" id="{id}-l">{label}</span>
  <div class="au-entries" role="group" aria-labelledby="{id}-l">
    {#each rows as row, i (i)}
      <div class="au-entry">
        <input class="d2-input" aria-label={t('m.auto.entry_key')} placeholder={t('m.auto.entry_key')} bind:value={row[0]} oninput={emit} />
        <input class="d2-input" aria-label={t('m.auto.entry_value')} placeholder={t('m.auto.entry_value')} bind:value={row[1]} oninput={emit} />
        <button type="button" class="d2-btn ghost icon" aria-label={t('m.auto.remove')} onclick={() => (rows.splice(i, 1), emit())}><Icon name="x-lg" /></button>
      </div>
    {/each}
    <div><button type="button" class="d2-btn sm" id="{id}-add" onclick={() => (rows.push(['', '']), emit())}><Icon name="plus-lg" />{t('m.auto.entry_add')}</button></div>
  </div>
</div>
