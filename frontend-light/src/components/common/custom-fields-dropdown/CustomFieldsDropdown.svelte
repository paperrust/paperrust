<!-- Port of common/custom-fields-dropdown/custom-fields-dropdown.component.html. Host <pngx-custom-fields-dropdown>
     in the parent markup. "Create new field" opens the classic UI (oncreate). -->
<script lang="ts">
  import '../../../styles/components/custom-fields-dropdown.css'
  import { tick } from 'svelte'
  import { t, msg } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import type { CustomField } from '../../../api/v1'
  import { session } from '../../../stores/settings.svelte'

  const DATA_TYPE_LABELS: Record<string, string> = {
    boolean: msg('common.boolean'),
    date: msg('common.date'),
    integer: msg('common.integer'),
    float: msg('common.number'),
    monetary: msg('common.monetary'),
    string: msg('common.text'),
    url: msg('common.url'),
    documentlink: msg('common.document_link'),
    select: msg('common.select'),
    longtext: msg('common.long_text'),
  }

  let {
    fields = [],
    disabled = false,
    existingFields = [],
    onadded,
    oncreate,
  }: {
    fields?: CustomField[]
    disabled?: boolean
    existingFields?: { field: number }[]
    onadded?: (f: CustomField) => void
    oncreate?: (name: string | undefined) => void
  } = $props()
  let filterText = $state<string | undefined>(undefined)
  let filterInput: HTMLInputElement | undefined = $state()
  let listEl: HTMLDivElement | undefined = $state()
  let keyboardIndex = -1
  const dd = new Dropdown({
    pngxPopper: true,
    onOpenChange: (open) => {
      if (open) setTimeout(() => filterInput?.focus(), 100)
      else filterText = undefined
    },
  })
  const unused = $derived(fields.filter((f) => !existingFields?.find((e) => e.field === f.id)))
  const filtered = $derived(unused.filter((f) => !filterText || f.name.toLowerCase().includes(filterText.toLowerCase())))
  const canCreate = $derived(session.can('add', 'customfield'))

  async function addField(f: CustomField) {
    onadded?.(f)
    await tick()
  }
  function enter() {
    if (filtered.length === 1) addField(filtered[0])
    else if (filterText && filtered.length === 0 && canCreate) oncreate?.(filterText)
  }
  function buttons(): HTMLButtonElement[] {
    return [...(listEl?.querySelectorAll('button') ?? [])] as HTMLButtonElement[]
  }
  function listKeyDown(e: KeyboardEvent) {
    const b = buttons()
    if (e.key === 'ArrowDown') {
      keyboardIndex = e.target instanceof HTMLInputElement ? 0 : Math.min(b.length - 1, keyboardIndex + 1)
      b[keyboardIndex]?.focus()
      e.preventDefault()
    } else if (e.key === 'ArrowUp') {
      if (keyboardIndex > 0) {
        keyboardIndex--
        b[keyboardIndex]?.focus()
      } else {
        keyboardIndex = -1
        filterInput?.focus()
      }
      e.preventDefault()
    }
  }
</script>

<div class={dd.placementClass} class:show={dd.open} use:dd.hostEl>
    <button type="button" class="btn btn-sm btn-outline-primary dropdown-toggle" id="customFieldsDropdown" {disabled} aria-label={t('common.custom_fields')} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
      <pngx-bs-icon name="ui-radios">{@html icon('ui-radios')}</pngx-bs-icon><div class="d-none d-lg-inline ms-1">{t('common.custom_fields')}</div>
    </button>
    {#if dd.open}
    <div aria-labelledby="customFieldsDropdown" class="dropdown-menu shadow custom-fields-dropdown show" use:dd.menuEl>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="list-group list-group-flush" onkeydown={listKeyDown} bind:this={listEl}>
            <div class="list-group-item">
                <div class="input-group input-group-sm">
                  <input class="form-control" type="text" bind:value={filterText} placeholder={t('ui.search_fields')} onkeyup={(e) => e.key === 'Enter' && enter()} bind:this={filterInput}>
                </div>
            </div>
            {#each filtered as field (field.id)}
                <button type="button" class="list-group-item list-group-item-action bg-light" onclick={() => addField(field)}>
                    <div class="d-flex">{field.name}{' '}<small class="ms-auto text-muted">{DATA_TYPE_LABELS[field.data_type] ? t(DATA_TYPE_LABELS[field.data_type]) : ''}</small></div>
                </button>
            {/each}
            {#if !filterText?.length || filtered.length === 0}
                <button type="button" class="list-group-item list-group-item-action bg-light" onclick={() => oncreate?.(filterText)} disabled={!canCreate}>
                    <pngx-bs-icon width=".9em" height=".9em" name="asterisk" class="me-1">{@html icon('asterisk', '.9em', '.9em')}</pngx-bs-icon>{t('ui.create_new_field')}
                </button>
            {/if}
        </div>
    </div>
    {/if}
</div>
