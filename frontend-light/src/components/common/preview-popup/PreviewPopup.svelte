<!-- Port of common/preview-popup/preview-popup.component.{html,ts}: link + ngbPopover (container="body",
     autoClose, popoverClass "shadow popover-preview", when preloading additionally "pe-none opacity-0").
     Host <pngx-preview-popup> in the parent markup; content (icon/text) as children.
     The popover content (pdf.js/<object>/text) is only loaded on the first hover (PreviewPopupContent.svelte),
     so that it is not part of the first load. Logic: preview-popup.ts. -->
<script lang="ts">
  import '../../../styles/components/preview-popup.css'
  import { onDestroy, type Snippet } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { api } from '../../../api/client'
  import { withSearchHash } from '../../../stores/searchQuery'
  import { Popover } from '../../../lib/popover.svelte'
  import { HoverPreview, type PreviewDoc } from './preview-popup'

  let {
    id = undefined,
    document: doc = undefined,
    link = undefined,
    linkClasses = 'btn btn-sm btn-outline-secondary',
    linkTarget = '_blank',
    linkTitle = undefined,
    children,
  }: {
    /** Document ID (without `document` title/MIME type are loaded on open) */
    id?: number
    /** upstream @Input document */
    document?: PreviewDoc
    link?: string
    linkClasses?: string
    linkTarget?: string
    linkTitle?: string
    children: Snippet
  } = $props()

  const docId = $derived((doc?.id ?? id) as number)
  let loaded = $state<PreviewDoc | undefined>()
  const shown = $derived(doc ?? (loaded?.id === docId ? loaded : undefined))

  const popover = new Popover({ autoClose: true, triggers: 'manual' })
  let hidden = $state(true)
  const hover = new HoverPreview({
    isOpen: () => popover.open,
    open: () => {
      hidden = true
      popover.show()
    },
    reveal: () => (hidden = false),
    close: () => popover.hide(),
  })
  onDestroy(() => hover.dispose())

  // container="body": window directly under <body> (only node of its {#if} block)
  function toBody(node: HTMLElement) {
    window.document.body.appendChild(node)
    return { destroy: () => node.remove() }
  }
  const loadContent = () => import('./PreviewPopupContent.svelte')
</script>

<a href={link ?? withSearchHash(api.previewUrl(docId))} class={linkClasses} target={linkTarget} title={linkTitle ?? t('ui.open_preview')} onclick={(e) => e.stopPropagation()} onmouseenter={() => hover.enter()} onmouseleave={() => hover.leave()} use:popover.triggerEl>
  {@render children()}
</a>
{#if popover.open}
  <ngb-popover-window role="tooltip" class="popover shadow popover-preview" class:pe-none={hidden} class:opacity-0={hidden} id={popover.id} use:toBody use:popover.windowEl><div class="popover-arrow" data-popper-arrow></div>{#if shown}<h3 class="popover-header">{shown.title || t('common.no_title')}</h3>{/if}<div class="popover-body">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="preview-popup-container" onmouseenter={() => hover.enter()} onmouseleave={() => hover.leave()}>
      {#await loadContent() then m}
        <m.default id={docId} document={shown} ondocument={(d: PreviewDoc) => (loaded = d)} />
      {/await}
    </div>
  </div></ngb-popover-window>
{/if}
