<!-- Port of common/share-links-dialog (incl. PaperRust hint in desktop mode, frontend/patches/09: pl-server
     sets <meta name="paperrust_share_mode" content="local|lan"> and with "lan" paperrust_share_url).
     PaperRust (pl-pdftools §4.1): hint which version is redacted, version choice for new links and the
     version per link – a link to the main document shows version 1, not the redacted version. -->
<script lang="ts">
  import '../../../styles/components/share-links-dialog.css'
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { api } from '../../../api/client'
  import type { ShareLink } from '../../../api/v1'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { docsUrl } from '../../../lib/docs-url'
  import { daysRemainingLabel, defaultShareVersion, expirationOptions, redactionNotice, shareMode, shareUrl, shareVersions, versionName, type ShareVersion } from './share-links'

  let { modal, documentId, hasArchiveVersion = true, title = t('common.share_links') }: { modal: ModalRef<any>; documentId: number; hasArchiveVersion?: boolean; title?: string } = $props()

  const paperrustShareMode = shareMode()
  const paperrustShareDocsUrl = docsUrl('paperrust/share-links')
  const options = expirationOptions()
  let shareLinks = $state<ShareLink[] | undefined>(undefined)
  let loading = $state(false)
  let copied = $state<number | null | undefined>(undefined)
  let expirationDays = $state<number | null>(7)
  // svelte-ignore state_referenced_locally
  let useArchiveVersion = $state(hasArchiveVersion)
  let versions = $state<ShareVersion[]>([])
  let shareVersionId = $state<number | undefined>(undefined)
  const notice = $derived(redactionNotice(versions))
  const redactState = (v: ShareVersion) => (v.redacted ? t('ui.share_is_redacted') : t('ui.share_not_redacted'))
  const versionOf = (link: ShareLink) => versions.find((v) => v.id === link.document)

  async function loadVersions() {
    try {
      const r = await api.getVersions(documentId)
      versions = shareVersions(r.versions, documentId)
      shareVersionId ??= defaultShareVersion(versions, documentId)
    } catch {
      versions = [] // without version list as before: link to the main document
    }
  }

  async function refresh() {
    loading = true
    try {
      shareLinks = await api.getShareLinks(documentId)
      loading = false
    } catch (e) {
      toastService.showError(t('ui.error_retrieving_links'), e)
    }
  }
  onMount(() => {
    loadVersions()
    refresh()
  })

  const getShareUrl = (link: ShareLink) => shareUrl(`${location.origin}/share/${link.slug}`)

  async function copy(link: ShareLink) {
    try {
      await navigator.clipboard.writeText(getShareUrl(link))
    } catch {
      return
    }
    copied = link.id
    setTimeout(() => (copied = null), 3000)
  }
  const canShare = (link: ShareLink) => !!(navigator?.canShare && navigator.canShare({ url: getShareUrl(link) }))
  const share = (link: ShareLink) => navigator.share({ url: getShareUrl(link) })

  async function del(link: ShareLink) {
    try {
      await api.deleteShareLink(link.id)
      refresh()
    } catch (e) {
      toastService.showError(t('ui.error_deleting_link'), e)
    }
  }

  async function createLink() {
    let expiration: Date | null = null
    if (expirationDays) {
      expiration = new Date()
      expiration.setDate(expiration.getDate() + expirationDays)
    }
    loading = true
    try {
      const result = await api.createShareLink(shareVersionId ?? documentId, useArchiveVersion ? 'archive' : 'original', expiration)
      loading = false
      setTimeout(() => copy(result), 10)
      refresh()
    } catch (e) {
      loading = false
      toastService.showError(t('ui.error_creating_link'), e)
    }
  }
</script>

<pngx-share-links-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body p-0">
  {#if paperrustShareMode === 'local'}
    <div class="alert alert-warning small m-2" role="note">
      {t('ui.links_only_work_computer')}
      <a class="ms-1" href={paperrustShareDocsUrl} target="_blank" rel="noopener noreferrer">{t('common.read_more')}</a>
    </div>
  {:else if paperrustShareMode === 'lan'}
    <div class="alert alert-info small m-2" role="note">
      {t('ui.sharing_in_local_network')}
      <a class="ms-1" href={paperrustShareDocsUrl} target="_blank" rel="noopener noreferrer">{t('common.read_more')}</a>
    </div>
  {/if}
  {#if notice}
    <div class="alert alert-warning small m-2" role="note" data-testid="redaction-notice">
      <strong>{t('ui.share_redacted_title')}</strong>
      <div>{t('ui.share_redacted_versions', { PH: notice.redacted.join(', ') })}</div>
      {#if notice.readable.length}
        <div>{t('ui.share_readable_versions', { PH: notice.readable.join(', ') })}</div>
      {/if}
      <div>{t('ui.share_link_shows_version')}</div>
    </div>
  {/if}
  <ul class="list-group list-group-flush">
    {#if !shareLinks || shareLinks.length === 0}
      <li class="list-group-item fst-italic small text-center text-secondary">
        {t('ui.no_existing_links')}
      </li>
    {/if}
    {#each shareLinks ?? [] as link (link.id)}
      <li class="list-group-item">
        <div class="input-group w-100">
          <input type="text" class="form-control" aria-label="Share link" value={getShareUrl(link)} readonly>
          {#if link.expiration}
            <span class="input-group-text">
              {daysRemainingLabel(link.expiration)}
            </span>
          {/if}
          <button type="button" class="btn btn-outline-primary" onclick={() => copy(link)}>
              {#if copied !== link.id}
                <pngx-bs-icon width="1.2em" height="1.2em" name="clipboard-fill">{@html icon('clipboard-fill', '1.2em', '1.2em')}</pngx-bs-icon>
              {/if}
              {#if copied === link.id}
                <pngx-bs-icon width="1.2em" height="1.2em" name="clipboard-check-fill">{@html icon('clipboard-check-fill', '1.2em', '1.2em')}</pngx-bs-icon>
              {/if}
              <span class="visually-hidden">{t('common.copy')}</span>
          </button>
          {#if canShare(link)}
            <button type="button" class="btn btn-outline-primary" onclick={() => share(link)}>
              <pngx-bs-icon width="1.2em" height="1.2em" name="box-arrow-up">{@html icon('box-arrow-up', '1.2em', '1.2em')}</pngx-bs-icon><span class="visually-hidden">{t('ui.share')}</span>
            </button>
          {/if}
          <button type="button" class="btn btn-outline-danger" onclick={() => del(link)}>
            <pngx-bs-icon width="1.2em" height="1.2em" name="trash">{@html icon('trash', '1.2em', '1.2em')}</pngx-bs-icon><span class="visually-hidden">{t('common.delete')}</span>
          </button>
        </div>
        {#if versions.length > 1 && versionOf(link)}
          {@const v = versionOf(link)!}
          <div class="small mt-1" class:text-success={notice && v.redacted} class:text-warning-emphasis={notice && !v.redacted} data-testid="link-version">
            {versionName(v)}{notice ? ` · ${redactState(v)}` : ''}
          </div>
        {/if}
        <span class="badge copied-badge bg-primary small fade ms-4 position-absolute top-50 translate-middle-y pe-none z-3" class:show={copied === link.id}>{t('ui.copied')}</span>
      </li>
    {/each}
  </ul>
</div>
<div class="modal-footer">
  {#if versions.length > 1}
    <div class="input-group w-100">
      <label class="input-group-text" for="shareVersion">{t('ui.share_version')}:</label>
      <select class="form-select fs-6" id="shareVersion" value={String(shareVersionId)} onchange={(e) => (shareVersionId = +e.currentTarget.value)}>
        {#each [...versions].reverse() as v (v.id)}
          <option value={String(v.id)}>{versionName(v)}{notice ? ` (${redactState(v)})` : ''}</option>
        {/each}
      </select>
    </div>
  {/if}
  <div class="input-group w-100">
    <div class="form-check form-switch ms-auto">
      <input class="form-check-input" type="checkbox" role="switch" id="versionSwitch" disabled={!hasArchiveVersion} bind:checked={useArchiveVersion}>
      <label class="form-check-label" for="versionSwitch">{t('ui.share_archive_version')}</label>
    </div>
  </div>
  <div class="input-group w-100 mt-2">
    <label class="input-group-text" for="addLink">{t('ui.expires')}:</label>
    <select class="form-select fs-6" value={String(expirationDays)} onchange={(e) => (expirationDays = e.currentTarget.value === 'null' ? null : +e.currentTarget.value)}>
      {#each options as option (option.value)}
        <option value={String(option.value)}>{option.label}</option>
      {/each}
    </select>
    <button class="btn btn-outline-primary ms-auto" type="button" onclick={createLink} disabled={loading}>
      {#if loading}
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>
      {/if}
      {#if !loading}
        <pngx-bs-icon name="plus">{@html icon('plus')}</pngx-bs-icon>
      {/if}
      {t('common.create')}
    </button>
  </div>
</div></pngx-share-links-dialog>
