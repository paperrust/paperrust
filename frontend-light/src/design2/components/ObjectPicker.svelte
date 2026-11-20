<!-- Selection from objects (tags, correspondents, …) with search; single or multiple, optionally "not assigned".
     Usable as a filter (chip button) and as a form field (field button). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { rank } from '../lib/palette'
  import Icon from './Icon.svelte'
  import Menu from './Menu.svelte'

  interface Opt {
    id: number
    name: string
    color?: string
  }
  let {
    options,
    selected,
    multi = false,
    allowNone = false,
    noneLabel = undefined,
    label,
    placeholder = '',
    variant = 'field',
    icon = undefined,
    disabled = false,
    id = undefined,
    onchange,
    oncreate = undefined,
  }: {
    options: Opt[]
    selected: (number | null)[]
    multi?: boolean
    allowNone?: boolean
    noneLabel?: string
    label: string
    placeholder?: string
    variant?: 'field' | 'chip'
    icon?: string
    disabled?: boolean
    id?: string
    onchange: (next: (number | null)[], toggled: number | null) => void
    /** "Create new" in the menu (with the search text as name); missing = no entry */
    oncreate?: (name: string) => void
  } = $props()

  let open = $state(false)
  let q = $state('')
  let hi = $state(0)
  const none = $derived(noneLabel ?? t('common.not_assigned'))
  const list = $derived(
    rank(
      options.map((o) => ({ ...o, id: String(o.id), oid: o.id, label: o.name, group: '' })),
      q,
      200,
    ),
  )
  const rows = $derived<{ oid: number | null; label: string; color?: string }[]>([...(allowNone && !q ? [{ oid: null, label: none }] : []), ...list.map((x) => ({ oid: x.oid, label: x.label, color: x.color }))])
  const names = $derived(selected.map((s) => (s === null ? none : (options.find((o) => o.id === s)?.name ?? `#${s}`))))
  const summary = $derived(names.length === 0 ? placeholder : names.length <= 2 ? names.join(', ') : `${names[0]} +${names.length - 1}`)

  function pick(oid: number | null) {
    if (multi) {
      const has = selected.includes(oid)
      onchange(has ? selected.filter((x) => x !== oid) : [...selected.filter((x) => (oid === null ? false : x !== null)), oid], oid)
    } else {
      onchange(selected[0] === oid ? [] : [oid], oid)
      open = false
    }
  }
  function onkey(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      hi = Math.min(rows.length - 1, hi + 1)
      e.preventDefault()
      e.stopPropagation()
    } else if (e.key === 'ArrowUp') {
      hi = Math.max(0, hi - 1)
      e.preventDefault()
      e.stopPropagation()
    } else if (e.key === 'Enter') {
      if (rows[hi]) pick(rows[hi].oid)
      e.preventDefault()
    }
  }
  $effect(() => {
    void q
    hi = 0
  })
</script>

<div class="picker">
  <button
    type="button"
    class={variant === 'chip' ? 'chipbtn' : 'd2-input fieldbtn'}
    class:on={variant === 'chip' && selected.length > 0}
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={`${label}: ${summary || t('m.ui.none')}`}
    {id}
    {disabled}
    onclick={() => {
      open = !open
      q = ''
    }}
  >
    {#if icon}<Icon name={icon} />{/if}
    <span class="sum" class:ph={!names.length}>{variant === 'chip' ? (names.length ? `${label}: ${summary}` : label) : summary}</span>
    <Icon name="chevron-down" />
  </button>
  {#if open}
    <Menu onclose={() => (open = false)} {label} align="left" width="280px">
      <!-- svelte-ignore a11y_autofocus -->
      <input class="d2-input" type="search" bind:value={q} placeholder={t('common.filter')} aria-label={t('m.ui.filter', { PH: label })} onkeydown={onkey} autofocus />
      <ul role="listbox" aria-multiselectable={multi} aria-label={label}>
        {#each rows as r, i (r.oid ?? 'none')}
          <li role="option" aria-selected={selected.includes(r.oid)} class:hi={i === hi}>
            <button type="button" class="d2-menu-item" onclick={() => pick(r.oid)} onpointermove={() => (hi = i)}>
              <span class="box" class:checked={selected.includes(r.oid)} class:radio={!multi} aria-hidden="true">{#if selected.includes(r.oid)}<Icon name="check-lg" />{/if}</span>
              {#if r.color}<span class="sw" style:background={r.color}></span>{/if}
              <span class="nm" class:none={r.oid === null}>{r.label}</span>
            </button>
          </li>
        {:else}
          <li class="d2-muted empty">{t('m.ui.no_results')}</li>
        {/each}
      </ul>
      {#if oncreate}
        <div class="d2-menu-sep"></div>
        <button type="button" class="d2-menu-item" data-create onclick={() => { const n = q.trim(); open = false; oncreate(n) }}><Icon name="plus-lg" />{q.trim() ? t('m.ui.create_named', { PH: q.trim() }) : t('m.ui.create_new')}</button>
      {/if}
      {#if selected.length}
        <div class="d2-menu-sep"></div>
        <button type="button" class="d2-menu-item" onclick={() => { onchange([], null); open = false }}><Icon name="x-circle" />{t('common.clear')}</button>
      {/if}
    </Menu>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    min-width: 0;
  }
  .chipbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 260px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r-pill);
    background: var(--d2-surface);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
    font-weight: 500;
    cursor: pointer;
  }
  .chipbtn:hover {
    background: var(--d2-surface-2);
  }
  .chipbtn.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
  }
  .fieldbtn {
    display: flex;
    align-items: center;
    gap: 6px;
    text-align: left;
    cursor: pointer;
  }
  .fieldbtn:disabled {
    cursor: default;
    opacity: 0.7;
  }
  .sum {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sum.ph {
    color: var(--d2-text-3);
  }
  ul {
    list-style: none;
    margin: var(--d2-s1) 0 0;
    padding: 0;
    max-height: 280px;
    overflow: auto;
  }
  .hi .d2-menu-item {
    background: var(--d2-surface-2);
  }
  .box {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    flex: none;
    border: 1.5px solid var(--d2-border-strong);
    border-radius: 4px;
    font-size: 11px;
  }
  .box.radio {
    border-radius: 50%;
  }
  .box.checked {
    border-color: var(--d2-accent);
    background: var(--d2-accent);
    color: var(--d2-on-accent);
  }
  .sw {
    width: 10px;
    height: 10px;
    flex: none;
    border-radius: 50%;
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nm.none {
    font-style: italic;
    color: var(--d2-text-2);
  }
  .empty {
    padding: var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
</style>
