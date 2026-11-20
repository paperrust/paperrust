<!-- Info button + ngbPopover from common/page-header/page-header.component.html (loaded lazily by PageHeader.svelte). -->
<script lang="ts">
  import '../../../styles/components/page-header.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import { Popover } from '../../../lib/popover.svelte'
  import { docsUrl } from '../../../lib/docs-url'
  let { info, infoLink = undefined }: { info: string; infoLink?: string } = $props()
  const infoPopover = new Popover({ autoClose: true })
</script>

<button class="btn btn-sm btn-link text-muted p-0 ms-2 p-md-2 ms-md-0" title={t("common.whats")} type="button" use:infoPopover.triggerEl>
  <pngx-bs-icon name="question-circle">{@html icon('question-circle')}</pngx-bs-icon>
</button>
{#if infoPopover.open}
  <ngb-popover-window role="tooltip" class="popover" id={infoPopover.id} use:infoPopover.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
    <p class:mb-0={!infoLink}>{@html info}</p>
    {#if infoLink}
      <a href={docsUrl(infoLink)} target="_blank" referrerpolicy={'noopener noreferrer' as any}>{t('common.read_more')}</a>
      <pngx-bs-icon class="ms-1" width=".8em" height=".8em" name="box-arrow-up-right">{@html icon('box-arrow-up-right', '.8em', '.8em')}</pngx-bs-icon>
    {/if}
  </div></ngb-popover-window>
{/if}
