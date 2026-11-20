<!-- Port of document-list/document-card-small/document-card-small.component.html. Host in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/document-card-small.css'
  import { t } from '../../../lib/i18n'
  import { formatDate } from '../../../lib/dates'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import type { Document } from '../../../api/v1'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import Tag from '../../common/tag/Tag.svelte'
  import PreviewPopup from '../../common/preview-popup/PreviewPopup.svelte'
  import CustomFieldDisplay from '../../common/custom-field-display/CustomFieldDisplay.svelte'
  import { router } from '../../../router.svelte'

  let {
    document: d,
    selected = false,
    priority = false,
    displayFields,
    dateFormat,
    dateLocale,
    ontoggle,
    onopen,
    onclicktag,
    onclickcorrespondent,
    onclickdocumenttype,
    onclickstoragepath,
  }: {
    document: Document
    selected?: boolean
    priority?: boolean
    displayFields: string[]
    dateFormat: string
    dateLocale: string
    ontoggle: (e: MouseEvent) => void
    onopen: () => void
    onclicktag: (id: number) => void
    onclickcorrespondent: (id: number) => void
    onclickdocumenttype: (id: number) => void
    onclickstoragepath: (id: number) => void
  } = $props()

  const has = (f: string) => displayFields.includes(f)
  const limit = $derived(d.notes.length > 0 ? 6 : 7)
  const moreTags = $derived(d.tags.length > limit ? d.tags.length - (limit - 1) : null)
  const tagIDs = $derived(d.tags.length > limit ? d.tags.slice(0, limit - 1) : d.tags)
  const imgAttrs = $derived<Record<string, string>>(priority ? { fill: '', priority: '' } : { fill: '' })
  const inverted = $derived(session.get(SETTINGS_KEYS.DARK_MODE_THUMB_INVERTED))
  const notesEnabled = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED))
  const title = $derived(d.title || t('common.no_title'))
  const stop = (fn: () => void) => (e: Event) => {
    fn()
    e.stopPropagation()
  }
</script>

<div class="col p-2 h-100 fade show">
  <div class="card h-100 shadow-sm document-card" class:card-selected={selected}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="border-bottom doc-img-container rounded-top" onclick={ontoggle} ondblclick={onopen}>
        <img class="card-img doc-img position-absolute w-100 h-100 top-0 start-0" class:inverted {...imgAttrs} src={api.thumbUrl(d.id)} alt={t('list.thumbnail', { INTERPOLATION: title })} loading={priority ? 'eager' : 'lazy'} fetchpriority={priority ? 'high' : 'auto'} decoding={priority ? 'sync' : 'auto'} sizes="100vw">

        <div class="border-end border-bottom bg-light py-1 px-2 document-card-check">
          <div class="form-check">
            <input type="checkbox" class="form-check-input" id="smallCardCheck{d.id}" checked={selected} onclick={ontoggle}>
            <label class="form-check-label" for="smallCardCheck{d.id}"></label>
          </div>
        </div>

      {#if has('tag')}
        <div class="tags d-flex flex-column text-end position-absolute me-1 fs-6" class:tags-no-wrap={d.tags.length > 3}>
          {#each tagIDs as tagID (tagID)}
            <pngx-tag><Tag tag={data.tags.get(tagID)} clickable linkTitle={t('list.toggle_tag_filter')} onclick={stop(() => onclicktag(tagID))} /></pngx-tag>
          {/each}
          {#if moreTags}
            <div>
              <span class="badge text-dark">+ {moreTags}</span>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    {#if has('note') && notesEnabled && d.notes.length}
      <a href={router.href(`documents/${d.id}/notes`)} class="document-card-notes py-2 px-1">
        <span class="badge rounded-pill bg-light border text-primary">
          <pngx-bs-icon width="1.2em" height="1.2em" class="ms-1 me-1" name="chat-left-text">{@html icon('chat-left-text', '1.2em', '1.2em')}</pngx-bs-icon>
        {d.notes.length}</span>
      </a>
    {/if}

    <div class="card-body bg-light p-2">
      <p class="card-text">
          {#if has('correspondent') && d.correspondent}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_missing_attribute -->
            <a title={t('list.toggle_correspondent_filter')} onclick={stop(() => onclickcorrespondent(d.correspondent!))} class="fw-bold btn-link">{data.correspondents.get(d.correspondent)?.name ?? ''}</a>
            {#if has('title')}:{/if}
          {/if}
          {#if has('title')}
            {title}
          {/if}
      </p>
    </div>
    <div class="card-footer pt-0 pb-2 px-2">
      <div class="list-group list-group-flush border-0 pt-1 pb-2 card-info">
          {#if has('documenttype') && d.document_type}
            <button type="button" class="list-group-item list-group-item-action bg-transparent ps-0 p-1 border-0 d-flex align-items-start" title={t('list.toggle_document_type_filter')}
              onclick={stop(() => onclickdocumenttype(d.document_type!))}>
              <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted flex-shrink-0" name="file-earmark">{@html icon('file-earmark', '1em', '1em')}</pngx-bs-icon>
              <small>{data.documentTypes.get(d.document_type)?.name ?? ''}</small>
            </button>
          {/if}
          {#if has('storagepath') && d.storage_path}
            <button type="button" class="list-group-item list-group-item-action bg-transparent ps-0 p-1 border-0 d-flex align-items-start" title={t('list.toggle_storage_path_filter')}
              onclick={stop(() => onclickstoragepath(d.storage_path!))}>
              <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted flex-shrink-0" name="folder">{@html icon('folder', '1em', '1em')}</pngx-bs-icon>
              <small>{data.storagePaths.get(d.storage_path)?.name ?? ''}</small>
            </button>
          {/if}
          {#if has('created')}
            <div class="list-group-item bg-transparent p-0 border-0 d-flex flex-wrap-reverse justify-content-between">
              <div class="ps-0 p-1" title={undefined}>
                <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="calendar-event">{@html icon('calendar-event', '1em', '1em')}</pngx-bs-icon>
                <small>{formatDate(d.created, 'mediumDate', dateLocale)}</small>
              </div>
            </div>
          {/if}
          {#if has('added')}
            <div class="list-group-item bg-transparent p-0 border-0 d-flex flex-wrap-reverse justify-content-between">
              <div class="ps-0 p-1">
                <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="calendar-event">{@html icon('calendar-event', '1em', '1em')}</pngx-bs-icon>
                <small>{formatDate(d.added, 'mediumDate', dateLocale)}</small>
              </div>
            </div>
          {/if}
          {#if has('pagecount') && d.page_count}
            <div class="list-group-item bg-transparent p-0 border-0 d-flex flex-wrap-reverse justify-content-between">
              <div class="ps-0 p-1">
                <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="files">{@html icon('files', '1em', '1em')}</pngx-bs-icon>
                <small>{t('list.pages', { VAR_PLURAL: d.page_count, INTERPOLATION: d.page_count })}</small>
              </div>
            </div>
          {/if}
          {#if has('asn') && typeof d.archive_serial_number === 'number'}
            <div class="ps-0 p-1">
              <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="upc-scan">{@html icon('upc-scan', '1em', '1em')}</pngx-bs-icon>
              <small>#{d.archive_serial_number}</small>
            </div>
          {/if}
          {#if has('owner') && d.owner && d.owner !== session.data?.user.id}
            <div class="ps-0 p-1">
              <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="person-fill-lock">{@html icon('person-fill-lock', '1em', '1em')}</pngx-bs-icon>
              <small>{data.username(d.owner)}</small>
            </div>
          {/if}
          {#if has('shared') && d.is_shared_by_requester}
            <div class="ps-0 p-1">
              <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="people-fill">{@html icon('people-fill', '1em', '1em')}</pngx-bs-icon>
              <small>{t('common.shared')}</small>
            </div>
          {/if}
          {#each d.custom_fields ?? [] as field (field.field)}
            {#if has('custom_field_' + field.field)}
              <div class="ps-0 p-1 d-flex align-items-center overflow-hidden">
                <pngx-bs-icon width="1em" height="1em" class="me-2 text-muted" name="ui-radios">{@html icon('ui-radios', '1em', '1em')}</pngx-bs-icon>
                <small><pngx-custom-field-display><CustomFieldDisplay document={d} fieldId={field.field} showNameIfEmpty /></pngx-custom-field-display></small>
              </div>
            {/if}
          {/each}
      </div>
      <div class="d-flex justify-content-between align-items-center">
        <div class="btn-group w-100">
            <a href={router.href(`documents/${d.id}`)} class="btn btn-sm btn-outline-secondary" title={t('common.open')}>
              <pngx-bs-icon name="file-earmark-richtext">{@html icon('file-earmark-richtext')}</pngx-bs-icon>
            </a>
            <pngx-preview-popup><PreviewPopup id={d.id}>
              <pngx-bs-icon name="eye">{@html icon('eye')}</pngx-bs-icon>
            </PreviewPopup></pngx-preview-popup>
            <a href={api.downloadUrl(d.id)} class="btn btn-sm btn-outline-secondary" title={t('common.download')} onclick={(e) => e.stopPropagation()}>
              <pngx-bs-icon name="download">{@html icon('download')}</pngx-bs-icon>
            </a>
        </div>
      </div>
    </div>
  </div>
</div>
