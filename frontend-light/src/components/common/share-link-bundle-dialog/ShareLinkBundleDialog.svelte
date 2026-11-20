<!-- Port of common/share-link-bundle-dialog (form → POST /api/share_link_bundles/, then result with link
     to copy). Creating including toasts is done here instead of in BulkEditorComponent.createShareLinkBundle (same
     behavior); "Manage share bundles" calls onmanage.
     PaperRust (pl-pdftools §4.1): hint for redacted versions, switch "use redacted versions". -->
<script lang="ts">
  import { t, locale } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import type { Document, ShareLinkBundle } from '../../../api/v1'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { formatDate } from '../../../lib/dates'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { fileSize } from '../../document-detail/document-detail'
  import { expirationOptions } from '../share-links-dialog/share-links'
  import { bundleIds, bundlePayload, copyText, docTitle, fileVersionLabel, redactedVersionMap, slugShareUrl, statusLabel } from './share-link-bundle'
  import { onMount } from 'svelte'

  let {
    modal,
    documents = [],
    onmanage,
  }: {
    modal: ModalRef<any>
    documents?: Pick<Document, 'id' | 'title'>[]
    onmanage?: () => void
  } = $props()

  const title = t('dialog.share_bundle.title')
  const btnCaption = t('dialog.share_bundle.create_link')
  const options = expirationOptions()
  const selectionCount = $derived(documents.length)
  const documentPreview = $derived(documents.slice(0, 10))
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  let shareArchiveVersion = $state(true)
  let expirationDays = $state<number | null>(7)
  let loading = $state(false)
  let buttonsEnabled = $state(true)
  let copied = $state(false)
  let cancelBtnCaption = $state(t('common.cancel'))
  let createdBundle = $state<ShareLinkBundle | null>(null)
  let redacted = $state<Map<number, number>>(new Map())
  let useRedacted = $state(true)

  onMount(async () => {
    if (!documents.length) return
    try {
      const r = await api.getFewDocuments(documents.map((d) => d.id))
      redacted = redactedVersionMap(r.results)
    } catch {
      redacted = new Map() // without version list as before
    }
  })

  async function submit() {
    if (createdBundle) return
    const payload = bundlePayload(
      bundleIds(documents.map((d) => d.id), redacted, useRedacted),
      shareArchiveVersion,
      expirationDays,
    )
    buttonsEnabled = false
    cancelBtnCaption = t('common.close')
    loading = true
    try {
      createdBundle = await api.createShareLinkBundle(payload)
      loading = false
      copied = false
      toastService.showInfo(t('dialog.share_bundle.requested_toast'))
    } catch (e) {
      loading = false
      buttonsEnabled = true
      toastService.showError(t('dialog.share_bundle.not_available'), e)
    }
  }
  async function copy(bundle: ShareLinkBundle) {
    if (await copyText(slugShareUrl(bundle.slug))) {
      copied = true
      setTimeout(() => (copied = false), 3000)
    }
  }
  function openManage() {
    modal.close()
    onmanage?.()
  }
</script>

<pngx-share-link-bundle-dialog><div class="modal-header">
  <h4 class="modal-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}></button>
</div>
<div class="modal-body">
  {#if !createdBundle}
    <form class="d-flex flex-column gap-3" onsubmit={(e) => e.preventDefault()}>
      <div>
        <p class="mb-1">
          {t('dialog.share_bundle.selected_documents')}
          {selectionCount}
        </p>
        {#if documentPreview.length > 0}
          <ul class="list-unstyled small mb-0">
            {#each documentPreview as doc (doc.id)}
              <li>
                <strong>{docTitle(doc.title)}</strong>
              </li>
            {/each}
            {#if selectionCount > documentPreview.length}
              <li>
                {t('dialog.share_bundle.more', { INTERPOLATION: selectionCount - documentPreview.length })}
              </li>
            {/if}
          </ul>
        {/if}
      </div>

      <div class="d-flex align-items-center justify-content-between">
        <div class="input-group">
          <label class="input-group-text" for="expirationDays">{t('ui.expires')}:</label>
          <select class="form-select" id="expirationDays" bind:value={expirationDays}>
            {#each options as option (option.value)}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
        </div>
        <div class="form-check form-switch w-100 ms-3">
          <input
            class="form-check-input"
            type="checkbox"
            role="switch"
            id="shareArchiveSwitch"
            bind:checked={shareArchiveVersion}
            aria-checked={shareArchiveVersion}
          />
          <label class="form-check-label" for="shareArchiveSwitch">{t('dialog.share_bundle.share_archive')}</label>
        </div>
      </div>
      {#if redacted.size}
        <div class="alert alert-warning small mb-0" role="note" data-testid="bundle-redaction-notice">
          <div>{t('ui.share_bundle_redacted', { PH: redacted.size })}</div>
          <div class="form-check form-switch mt-2">
            <input class="form-check-input" type="checkbox" role="switch" id="useRedactedSwitch" bind:checked={useRedacted} />
            <label class="form-check-label" for="useRedactedSwitch">{t('ui.share_bundle_use_redacted')}</label>
          </div>
          {#if !useRedacted}
            <div class="mt-1 fw-semibold">{t('ui.share_bundle_original_warning')}</div>
          {/if}
        </div>
      {/if}
    </form>
  {:else}
    <div class="d-flex flex-column gap-3">
      <div class="alert alert-success mb-0" role="status">
        <h6 class="alert-heading mb-1">{t('dialog.share_bundle.requested_heading')}</h6>
        <p class="mb-0 small">
          {t('dialog.share_bundle.requested_info')}
        </p>
      </div>
      <dl class="row mb-0 small">
        <dt class="col-sm-4">{t('common.status')}</dt>
        <dd class="col-sm-8">
          <span class="badge text-bg-secondary text-uppercase">{statusLabel(createdBundle.status)}</span>
        </dd>
        <dt class="col-sm-4">{t('dialog.share_bundle.slug')}</dt>
        <dd class="col-sm-8"><code>{createdBundle.slug}</code></dd>
        <dt class="col-sm-4">{t('dialog.share_bundle.link')}</dt>
        <dd class="col-sm-8 position-relative">
          <label class="visually-hidden" for="shareBundleLink">{t('dialog.share_bundle.share_link')}</label>
          <div class="input-group input-group-sm">
            <input id="shareBundleLink" class="form-control" type="text" value={slugShareUrl(createdBundle.slug)} readonly>
            <button
              class="btn btn-outline-primary"
              type="button"
              onclick={() => copy(createdBundle!)}
            >
              {#if copied}
                <pngx-bs-icon name="clipboard-check">{@html icon('clipboard-check')}</pngx-bs-icon>
              {:else}
                <pngx-bs-icon name="clipboard">{@html icon('clipboard')}</pngx-bs-icon>
              {/if}
              <span class="visually-hidden">{t('dialog.share_bundle.copy_link')}</span>
            </button>
          </div>
          <span class="badge bg-primary fade position-absolute top-50 end-0 translate-middle-y me-5 pe-none z-3" class:show={copied}>{t('ui.copied')}</span>
        </dd>
        <dt class="col-sm-4">{t('common.documents_2')}</dt>
        <dd class="col-sm-8">{createdBundle.document_count}</dd>
        <dt class="col-sm-4">{t('ui.expires')}</dt>
        <dd class="col-sm-8">
          {#if createdBundle.expiration}
            {formatDate(createdBundle.expiration, 'short', dateLocale)}
          {:else}
            <span>{t('ui.never')}</span>
          {/if}
        </dd>
        <dt class="col-sm-4">{t('dialog.share_bundle.file_version')}</dt>
        <dd class="col-sm-8">{fileVersionLabel(createdBundle.file_version)}</dd>
        {#if createdBundle.size_bytes !== undefined && createdBundle.size_bytes !== null}
          <dt class="col-sm-4">{t('dialog.share_bundle.size')}</dt>
          <dd class="col-sm-8">{fileSize(createdBundle.size_bytes)}</dd>
        {/if}
      </dl>
    </div>
  {/if}
</div>
<div class="modal-footer">
  <div class="d-flex align-items-center gap-2 w-100">
    <div class="text-light fst-italic small">
      {t('dialog.share_bundle.zip_info')}
    </div>
    <button type="button" class="btn btn-outline-secondary btn-sm ms-auto" onclick={() => modal.dismiss()}>{cancelBtnCaption}</button>
    {#if createdBundle}
      <button type="button" class="btn btn-outline-secondary btn-sm text-nowrap" onclick={openManage}>{t('list.manage_share_link_bundles')}</button>
    {/if}

    {#if !createdBundle}
      <button
        type="button"
        class="btn btn-primary btn-sm d-inline-flex align-items-center gap-2 text-nowrap"
        onclick={submit}
        disabled={loading || !buttonsEnabled}>
        {#if loading}
          <span class="spinner-border spinner-border-sm" role="status" aria-hidden="true"></span>
        {/if}
        {btnCaption}
      </button>
    {/if}
  </div>
</div></pngx-share-link-bundle-dialog>
