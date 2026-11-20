<!-- Port of document-list/document-card-large/document-card-large.component.html. Host in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/document-card-large.css'
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
  import { sanitizeHighlight, searchScoreClass, searchNoteHighlights, hasSearchHighlights, shouldShowContentFallback, type SearchHit } from './search-hit'

  let {
    document: d,
    selected = false,
    priority = false,
    displayFields,
    dateLocale,
    ontoggle,
    onopen,
    onclicktag,
    onclickcorrespondent,
    onclickdocumenttype,
    onclickstoragepath,
    onclickmorelike,
  }: {
    document: Document
    selected?: boolean
    priority?: boolean
    displayFields: string[]
    dateLocale: string
    ontoggle: (e: MouseEvent) => void
    onopen: () => void
    onclicktag: (id: number) => void
    onclickcorrespondent: (id: number) => void
    onclickdocumenttype: (id: number) => void
    onclickstoragepath: (id: number) => void
    onclickmorelike: () => void
  } = $props()

  const has = (f: string) => displayFields.includes(f)
  const imgAttrs = $derived<Record<string, string>>(priority ? { fill: '', priority: '' } : { fill: '' })
  const inverted = $derived(session.get(SETTINGS_KEYS.DARK_MODE_THUMB_INVERTED))
  const notesEnabled = $derived(session.get(SETTINGS_KEYS.NOTES_ENABLED))
  const title = $derived(d.title || t('common.no_title'))
  const hit = $derived((d as Document & { __search_hit__?: SearchHit | null }).__search_hit__)
  const noteHighlights = $derived(searchNoteHighlights({ __search_hit__: hit }))
  const content = $derived((d.content ?? '').substring(0, 500) + ((d.content ?? '').length > 500 ? '...' : ''))
  const stop = (fn: () => void) => (e: Event) => {
    fn()
    e.stopPropagation()
  }
</script>

<div class="card document-card-large mb-3 shadow-sm bg-light placeholder-glow fade show document-card" class:card-selected={selected}>
  <div class="row g-0">
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="col-md-2 doc-img-container rounded-start" onclick={ontoggle} ondblclick={onopen}>
          <img {...imgAttrs} class="card-img doc-img border-end rounded-start position-absolute w-100 h-100 top-0 start-0" class:inverted src={api.thumbUrl(d.id)} alt={t('list.thumbnail', { INTERPOLATION: title })} loading={priority ? 'eager' : 'lazy'} fetchpriority={priority ? 'high' : 'auto'} decoding={priority ? 'sync' : 'auto'} sizes="100vw">

        <div class="border-end border-bottom bg-light document-card-check">
          <div class="form-check">
            <input type="checkbox" class="form-check-input" id="smallCardCheck{d.id}" checked={selected} onclick={ontoggle}>
            <label class="form-check-label" for="smallCardCheck{d.id}"></label>
          </div>
        </div>

    </div>
    <div class="col col-md-10">
      <div class="card-body">
        <div class="d-flex justify-content-between align-items-center">
          <h5 class="card-title w-100">
              {#if has('correspondent') && d.correspondent}
                  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_missing_attribute -->
                  <a title={t('common.filter_correspondent')} onclick={stop(() => onclickcorrespondent(d.correspondent!))} class="fw-bold btn-link">{data.correspondents.get(d.correspondent)?.name ?? ''}</a>
                {#if has('title')}:{/if}
              {/if}
              {#if has('title')}
                {title}
              {/if}
              {#if has('tag')}
                {#each d.tags as tagID (tagID)}
                  <pngx-tag class="ms-1"><Tag tag={data.tags.get(tagID)} clickable linkTitle={t('common.filter_tag')} onclick={stop(() => onclicktag(tagID))} /></pngx-tag>
                {/each}
              {/if}
          </h5>
        </div>
        <p class="card-text">
            {#if hasSearchHighlights({ __search_hit__: hit })}
              <span>{@html sanitizeHighlight(hit!.highlights)}</span>
            {/if}
            {#each noteHighlights as highlight, i (i)}
              <span class="d-block">
                <pngx-bs-icon name="chat-left-text">{@html icon('chat-left-text')}</pngx-bs-icon>
                <span>{@html sanitizeHighlight(highlight)}</span>
              </span>
            {/each}
            {#if shouldShowContentFallback({ __search_hit__: hit })}
              <span class="result-content">{content}</span>
            {/if}
        </p>

        <div class="d-flex flex-column flex-md-row align-items-md-center">
          <div class="btn-group">
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_missing_attribute -->
              <a class="btn btn-sm btn-outline-secondary" onclick={onclickmorelike}>
                <pngx-bs-icon name="diagram-3" class="me-1">{@html icon('diagram-3')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.more_like')}</span>
              </a>
              <a href={router.href(`documents/${d.id}`)} class="btn btn-sm btn-outline-secondary">
                <pngx-bs-icon name="file-earmark-richtext" class="me-1">{@html icon('file-earmark-richtext')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.open')}</span>
              </a>
              <pngx-preview-popup><PreviewPopup id={d.id}>
                <pngx-bs-icon name="eye" class="me-1">{@html icon('eye')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.view')}</span>
              </PreviewPopup></pngx-preview-popup>
              <a class="btn btn-sm btn-outline-secondary" href={api.downloadUrl(d.id)}>
                <pngx-bs-icon name="download" class="me-1">{@html icon('download')}</pngx-bs-icon><span class="d-none d-md-inline">{t('common.download')}</span>
              </a>
          </div>

          <div class="list-group list-group-horizontal border-0 card-info ms-md-auto mt-2 mt-md-0">
              {#if has('note') && notesEnabled && d.notes.length}
                <button onclick={() => (router.navigate(`documents/${d.id}/notes`))} class="list-group-item btn btn-sm bg-light text-dark p-1 border-0 me-2 d-flex align-items-center" title={t('list.view_notes')}>
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="chat-left-text">{@html icon('chat-left-text', '.9em', '.9em')}</pngx-bs-icon><small>{d.notes.length} Notes</small>
                </button>
              {/if}
              {#if has('documenttype') && d.document_type}
                <button type="button" class="list-group-item btn btn-sm bg-light text-dark p-1 border-0 me-2 d-flex align-items-center" title={t('common.filter_document_type')}
                  onclick={stop(() => onclickdocumenttype(d.document_type!))}>
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="file-earmark">{@html icon('file-earmark', '.9em', '.9em')}</pngx-bs-icon><small>{data.documentTypes.get(d.document_type)?.name ?? ''}</small>
                </button>
              {/if}
              {#if has('storagepath') && d.storage_path}
                <button type="button" class="list-group-item btn btn-sm bg-light text-dark p-1 border-0 me-2 d-flex align-items-center" title={t('common.filter_storage_path')}
                  onclick={stop(() => onclickstoragepath(d.storage_path!))}>
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="archive">{@html icon('archive', '.9em', '.9em')}</pngx-bs-icon><small>{data.storagePaths.get(d.storage_path)?.name ?? ''}</small>
                </button>
              {/if}
              {#if has('asn') && typeof d.archive_serial_number === 'number'}
                <div class="list-group-item me-2 bg-light text-dark p-1 border-0 d-flex align-items-center">
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="upc-scan">{@html icon('upc-scan', '.9em', '.9em')}</pngx-bs-icon><small>#{d.archive_serial_number}</small>
                </div>
              {/if}
              {#if has('created') || has('added')}
                {#if has('created')}
                  <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                    <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="calendar-event">{@html icon('calendar-event', '.9em', '.9em')}</pngx-bs-icon><small>{formatDate(d.created, 'mediumDate', dateLocale)}</small>
                  </div>
                {/if}
                {#if has('added')}
                  <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                    <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="calendar-event">{@html icon('calendar-event', '.9em', '.9em')}</pngx-bs-icon><small>{formatDate(d.added, 'mediumDate', dateLocale)}</small>
                  </div>
                {/if}
              {/if}
              {#if has('pagecount') && d.page_count}
                <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="files">{@html icon('files', '.9em', '.9em')}</pngx-bs-icon>
                  <small>{t('list.pages', { VAR_PLURAL: d.page_count, INTERPOLATION: d.page_count })}</small>
                </div>
              {/if}
              {#if has('owner') && d.owner && d.owner !== session.data?.user.id}
                <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="person-fill-lock">{@html icon('person-fill-lock', '.9em', '.9em')}</pngx-bs-icon><small>{data.username(d.owner)}</small>
                </div>
              {/if}
              {#if has('shared') && d.is_shared_by_requester}
                <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                  <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="people-fill">{@html icon('people-fill', '.9em', '.9em')}</pngx-bs-icon><small>{t('common.shared')}</small>
                </div>
              {/if}
              {#if hit?.score}
                <div class="list-group-item bg-light text-dark border-0 d-flex p-0 ps-4 search-score">
                  <small class="me-2 text-muted">{t('list.score')}</small>
                  <ngb-progressbar class="search-score-bar mx-2 mt-1 progress" role="progressbar" aria-valuenow={hit.score} aria-valuemin="0" aria-valuemax="1"><div class="progress-bar text-bg-{searchScoreClass({ __search_hit__: hit })}" style:width="{Math.min(Math.max(hit.score, 0), 1) * 100}%"></div></ngb-progressbar>
                </div>
              {/if}
              {#each d.custom_fields ?? [] as field (field.field)}
                {#if has('custom_field_' + field.field)}
                  <div class="list-group-item bg-light text-dark p-1 border-0 d-flex align-items-center">
                    <pngx-bs-icon width=".9em" height=".9em" class="me-2 text-muted" name="ui-radios">{@html icon('ui-radios', '.9em', '.9em')}</pngx-bs-icon>
                    <small>
                      <pngx-custom-field-display><CustomFieldDisplay document={d} fieldId={field.field} showNameIfEmpty /></pngx-custom-field-display>
                    </small>
                  </div>
                {/if}
              {/each}
          </div>
        </div>
      </div>
    </div>
  </div>
</div>
