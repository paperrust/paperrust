<!-- Port of common/page-header/page-header.component.html (title, ID badge). Host <pngx-page-header> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/page-header.css'
  import type { Component, Snippet } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  let {
    title,
    id = undefined,
    subTitle = undefined,
    info = undefined,
    infoLink = undefined,
    loading = false,
    children,
  }: { title: string; id?: number; subTitle?: string; info?: string; infoLink?: string; loading?: boolean; children?: Snippet } = $props()
  // Load the info popover (ngbPopover) only when a page needs it – keeps the Popper popover out of the first call
  let Info = $state<Component<{ info: string; infoLink?: string }> | null>(null)
  $effect(() => {
    if (info && !Info) import('./PageHeaderInfo.svelte').then((m) => (Info = m.default))
  })
  let copied = $state(false)
  let timer: ReturnType<typeof setTimeout> | undefined
  function copyID() {
    navigator.clipboard?.writeText(String(id)).then(() => {
      copied = true
      clearTimeout(timer)
      timer = setTimeout(() => (copied = false), 3000)
    })
  }
</script>

<div class="row pt-3 pb-2 align-items-center">
  <div class="col-md text-truncate">
    <h3 class="d-flex align-items-center mb-2 mb-md-1 pr-lh-14">
      <span class="d-flex align-items-center mw-100">
        <span class="text-truncate">{title}</span>
        {#if id}
          <button type="button" class="badge border-0 bg-primary text-primary-text-contrast ms-3 small fs-normal flex-shrink-0 text-nowrap" onclick={copyID} title={t('ui.copy_id')}>
            {#if copied}
              <pngx-bs-icon width="1em" height="1em" name="clipboard-check" class="me-1">{@html icon('clipboard-check', '1em', '1em')}</pngx-bs-icon>{t('ui.copied')}
            {:else}
              ID: {id}
            {/if}
          </button>
        {/if}
      </span>
      {#if subTitle}
        <span class="page-subtitle h6 mb-0 mt-1 fw-normal ms-md-3 text-truncate pr-lh-14">{subTitle}</span>
      {/if}
      {#if info && Info}
        <Info {info} {infoLink} />
      {/if}
      {#if loading}
        <output class="spinner-border spinner-border-sm fs-6 fw-normal" aria-hidden="true"><span class="visually-hidden">{t('common.loading')}</span></output>
      {/if}
    </h3>
  </div>
  <div class="btn-toolbar col col-md-auto gap-2">
    {@render children?.()}
  </div>
</div>
