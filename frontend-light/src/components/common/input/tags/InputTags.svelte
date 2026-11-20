<!-- Port of common/input/tags/tags.component.html. Host <pngx-input-tags> in the parent markup.
     Creating a new tag reports oncreate (parent component opens the classic UI). -->
<script lang="ts">
  import '../../../../styles/components/input-tags.css'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import type { Tag as TagT } from '../../../../api/v1'
  import NgSelect from '../../ng-select/NgSelect.svelte'
  import Tag from '../../tag/Tag.svelte'
  import { addWithParents, parentChain, removeWithChildren } from './tags'
  let {
    value = $bindable([]),
    tags = [],
    title = t('common.tags'),
    disabled = false,
    hint = undefined,
    suggestions = undefined,
    allowCreate = true,
    hideAddButton = false,
    showFilter = false,
    horizontal = false,
    multiple = true,
    onchange,
    oncreate,
    onfilterdocuments,
  }: {
    value?: number[]
    tags?: TagT[]
    title?: string
    disabled?: boolean
    hint?: string
    suggestions?: number[]
    allowCreate?: boolean
    hideAddButton?: boolean
    showFilter?: boolean
    horizontal?: boolean
    multiple?: boolean
    onchange?: (v: number[]) => void
    oncreate?: () => void
    onfilterdocuments?: (tags: TagT[]) => void
  } = $props()
  const getTag = (id: number) => tags.find((x) => x.id == id)
  const getSuggestions = () => (suggestions && tags ? suggestions.filter((id) => !value.includes(id)).map((id) => getTag(id)) : [])
  const hasPrivate = $derived(value.some((id) => !tags.find((x) => x.id === id)))
  function set(v: number[]) {
    value = v
    onchange?.(value)
  }
  function removeTag(id: number) {
    if (disabled) return
    set(removeWithChildren(value, id, tags))
  }
  function changed(v: number[]) {
    // ng-select (add): also add parents
    const added = v.filter((id) => !value.includes(id))
    let next = v
    for (const id of added) next = addWithParents(next, id, tags)
    set(next)
  }
  function addTag(e: Event, id: number) {
    e.preventDefault()
    set(addWithParents([...value, id], id, tags))
  }
</script>

<div class="mb-3 paperless-input-select paperless-input-tags" class:disabled class:pb-3={getSuggestions().length > 0}>
  <div class="row">
    {#if title}
      <div class="d-flex align-items-center" class:col-md-3={horizontal}>
        <label class="form-label" class:mb-md-0={horizontal} for="tags">{title}</label>
      </div>
    {/if}
    <div class="position-relative" class:col-md-9={horizontal}>
      <div class="input-group flex-nowrap">
        <NgSelect name="tags" items={tags} bindLabel="name" bindValue="id" {value} onchange={changed}
          {disabled}
          {multiple}
          closeOnSelect={false}
          hideSelected={tags.length > 0}
          taggable={allowCreate}>
          {#snippet label(item)}
            <button class="tag-wrap btn p-0 d-flex align-items-center" onclick={() => removeTag(item.id)} onmousedown={(e) => e.stopImmediatePropagation()} type="button" title={t('common.remove_tag')}>
              <pngx-bs-icon name="x" style={"margin-inline-end: 1px;"}>{@html icon('x')}</pngx-bs-icon>
              {#if item.id && tags}
                <pngx-tag style={"background-color: none;"}><Tag tag={getTag(item.id)} /></pngx-tag>
              {/if}
            </button>
          {/snippet}
          {#snippet option(item)}
            <div class="tag-option-row d-flex align-items-center" class:w-auto={!getTag(item.id)?.parent}>
              {#if item.id && tags}
                {#if getTag(item.id)?.parent}
                  <pngx-bs-icon name="list-nested" class="me-1">{@html icon('list-nested')}</pngx-bs-icon>
                  <span class="hierarchy-reveal d-flex align-items-center">
                    <span class="parents d-flex align-items-center">
                      {#each parentChain(item.id, tags) as p (p.id)}
                        <span class="badge me-1" style:background={p.color} style:color={p.text_color}>{p.name}</span>
                        <pngx-bs-icon name="chevron-right" width=".8em" height=".8em" class="me-1">{@html icon('chevron-right', '.8em', '.8em')}</pngx-bs-icon>
                      {/each}
                    </span>
                  </span>
                {/if}
                <pngx-tag class="current-tag d-flex"><Tag tag={getTag(item.id)} /></pngx-tag>
              {/if}
            </div>
          {/snippet}
        </NgSelect>
        {#if allowCreate && !hideAddButton}
          <button class="btn btn-outline-secondary" type="button" onclick={() => oncreate?.()} {disabled} aria-label={t('common.create_new_tag')}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="plus">{@html icon('plus', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
        {/if}
        {#if showFilter}
          <button class="btn btn-outline-secondary" type="button" onclick={() => onfilterdocuments?.(tags.filter((x) => value.includes(x.id)))} disabled={hasPrivate || value === null} title={t('input.filter_documents_tags')}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="filter">{@html icon('filter', '1.2em', '1.2em')}</pngx-bs-icon>
          </button>
        {/if}
      </div>
      {#if hint}
        <small class="form-text text-muted">{hint}</small>
      {/if}
      {#if getSuggestions().length > 0}
        <small class="position-absolute top-100">
          <span>{t('input.suggestions_label')}</span>&nbsp;
          {#each getSuggestions() as tag (tag?.id)}
            <a onclick={(e) => addTag(e, tag!.id)} href="">{tag?.name}</a>&nbsp;
          {/each}
        </small>
      {/if}
    </div>
  </div>
</div>
