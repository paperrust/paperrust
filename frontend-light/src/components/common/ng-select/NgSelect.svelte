<!-- Replica of <ng-select> (@ng-select/ng-select 24) with the same DOM and the same classes, so that the
     ng-select theme applies unchanged. Single/multiple selection, search, clearing; panel in the CDK overlay below/above the field.
     This component renders the host <ng-select> itself (classes depend on state); the encapsulation attribute
     of the parent component (sc-…) is set by tools/ng-markup.js as a prop, it is transferred to the host. -->
<script lang="ts" generics="T extends Record<string, any>">
  import '../../../styles/parts/select.css'
  import '../../../styles/parts/overlay.css'
  import { t } from '../../../lib/i18n'
  import { tick } from 'svelte'
  import { cdkOverlay } from '../../../lib/cdk-overlay'
  let {
    items = [],
    value = $bindable(),
    bindLabel = 'name',
    bindValue = 'id',
    multiple = false,
    clearable = true,
    searchable = true,
    placeholder = '',
    name = undefined,
    class: cls = '',
    disabled = false,
    autofocus = false,
    taggable = false,
    hideSelected = false,
    closeOnSelect = !multiple,
    notFoundText = undefined,
    onchange,
    option,
    label,
    ...rest
  }: {
    items?: T[]
    value?: any
    bindLabel?: string
    bindValue?: string
    multiple?: boolean
    clearable?: boolean
    searchable?: boolean
    placeholder?: string
    name?: string
    class?: string
    disabled?: boolean
    autofocus?: boolean
    /** [addTag] set → class ng-select-taggable (creating itself is handled by the parent component) */
    taggable?: boolean
    hideSelected?: boolean
    closeOnSelect?: boolean
    notFoundText?: string
    onchange?: (v: any) => void
    option?: import('svelte').Snippet<[T]>
    /** ng-label-tmp: replaces the content of .ng-value (parameters: entry, remove function) */
    label?: import('svelte').Snippet<[T, () => void]>
    [attr: string]: any
  } = $props()

  let open = $state(false)
  let focused = $state(autofocus)
  let term = $state('')
  let input: HTMLInputElement
  let marked = $state(0)

  const selectedValues = $derived(multiple ? ((value as any[]) ?? []) : value == null ? [] : [value])
  const selectedItems = $derived(selectedValues.map((v) => items.find((i) => i[bindValue] === v)).filter(Boolean) as T[])
  const filtered = $derived(
    (term ? items.filter((i) => String(i[bindLabel] ?? '').toLowerCase().includes(term.toLowerCase())) : items).filter(
      (i) => !hideSelected || !selectedValues.includes(i[bindValue]),
    ),
  )
  const hasValue = $derived(selectedItems.length > 0)

  $effect(() => {
    if (autofocus) input?.focus()
  })

  function select(i: T) {
    const v = i[bindValue]
    if (multiple) value = selectedValues.includes(v) ? selectedValues.filter((x) => x !== v) : [...selectedValues, v]
    else value = v
    term = ''
    if (closeOnSelect) open = false
    onchange?.(value)
  }
  function clear(e: Event) {
    e.stopPropagation()
    value = multiple ? [] : null
    onchange?.(value)
  }
  function toggle(e: MouseEvent) {
    if (disabled) return
    e.stopPropagation()
    open = !open
    input?.focus()
  }
  // Panel like ng-select 24 in the CDK overlay (lib/cdk-overlay.ts); on open markSelectedOrDefault + scrollTo(marked, true)
  let hostEl = $state<HTMLElement>()
  let placement = $state<'bottom' | 'top'>('bottom')
  let wasOpen = false
  $effect(() => {
    if (open && !wasOpen) {
      let last = -1
      filtered.forEach((it, i) => {
        if (selectedValues.includes(it[bindValue])) last = i
      })
      marked = last > -1 ? last : 0
      tick().then(() => {
        const panel = scrollHost?.querySelectorAll<HTMLElement>('.ng-option')[marked]
        if (!scrollHost || !panel) return
        const top = panel.offsetTop
        const ph = scrollHost.clientHeight
        if (!(ph >= top + panel.clientHeight)) scrollHost.scrollTop = top
      })
    }
    wasOpen = open
  })
  let scrollHost = $state<HTMLElement>()
  /** ng-select filter(): after filtering mark the selected entry, otherwise the first (markSelectedOrDefault) */
  function onSearchInput() {
    open = true
    const i = filtered.findIndex((it) => selectedValues.includes(it[bindValue]))
    marked = i > -1 ? i : 0
  }
  function keydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      open = true
      marked = Math.min(marked + 1, filtered.length - 1)
      e.preventDefault()
    } else if (e.key === 'ArrowUp') {
      marked = Math.max(marked - 1, 0)
      e.preventDefault()
    } else if (e.key === 'Enter') {
      // ng-select _handleEnter: select or open the marked entry; never pass Enter on to the form
      if (open && filtered[marked]) select(filtered[marked])
      else open = true
      e.preventDefault()
    } else if (e.key === 'Escape') open = false
  }
</script>

<svelte:window onclick={() => (open = false)} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<ng-select bind:this={hostEl} {...rest} {name} class="{cls} ng-select" class:ng-select-single={!multiple} class:ng-select-multiple={multiple} class:ng-select-searchable={searchable} class:ng-select-clearable={clearable} class:ng-select-taggable={taggable} class:ng-select-opened={open} class:ng-select-bottom={open && placement === 'bottom'} class:ng-select-top={open && placement === 'top'} class:ng-select-focused={focused} class:ng-select-disabled={disabled} class:ng-has-value={hasValue} class:ng-untouched={true} class:ng-pristine={true} class:ng-valid={true} onclick={(e) => e.stopPropagation()}><div class="ng-select-container" class:ng-has-value={hasValue} onmousedown={toggle}><div class="ng-value-container">{#if !hasValue || multiple}{#if !hasValue}<div class="ng-placeholder">{placeholder}</div>{/if}{/if}{#each selectedItems as it}<div class="ng-value">{#if label}{@render label(it, () => select(it))}{:else}{#if multiple}<span aria-hidden="true" class="ng-value-icon left" onmousedown={(e) => { e.stopPropagation(); select(it) }}>×</span>{/if}<span class="ng-value-label">{it[bindLabel]}</span>{/if}</div>{/each}<div class="ng-input"><input bind:this={input} aria-autocomplete="list" role="combobox" type="text" autocorrect="off" autocapitalize="off" autocomplete="off" aria-expanded={open} readonly={!searchable || undefined} {disabled} bind:value={term} oninput={onSearchInput} onfocus={() => (focused = true)} onblur={() => (focused = false)} onkeydown={keydown}></div></div>{#if clearable && hasValue && !disabled}<span class="ng-clear-wrapper" title="Clear all" onmousedown={clear}><span aria-hidden="true" class="ng-clear">×</span></span>{/if}<span class="ng-arrow-wrapper"><span class="ng-arrow"></span></span></div>{#if open && hostEl}<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions --><ng-dropdown-panel class="ng-dropdown-panel opacity-100" class:ng-select-bottom={placement === 'bottom'} class:ng-select-top={placement === 'top'} onclick={(e) => e.stopPropagation()} use:cdkOverlay={{ anchor: hostEl, onplacement: (p) => (placement = p) }}><div role="listbox" class="ng-dropdown-panel-items scroll-host" aria-label="Options List" bind:this={scrollHost}><div></div><div>{#each filtered as it, i}<div class="ng-option" role="option" aria-selected={selectedValues.includes(it[bindValue])} class:ng-option-marked={i === marked} class:ng-option-selected={selectedValues.includes(it[bindValue])} onmousedown={(e) => { e.preventDefault(); select(it) }} onmouseenter={() => (marked = i)}>{#if option}{@render option(it)}{:else}<span class="ng-option-label">{it[bindLabel]}</span>{/if}</div>{:else}<div class="ng-option ng-option-disabled">{notFoundText ?? t('ui.no_items_found')}</div>{/each}</div></div></ng-dropdown-panel>{/if}<div aria-atomic="true" aria-live="polite" role="status" class="ng-visually-hidden"></div></ng-select>
