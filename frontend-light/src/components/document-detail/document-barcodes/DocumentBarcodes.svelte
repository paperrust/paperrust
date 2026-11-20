<!-- Port of document-detail/document-barcodes/document-barcodes.component.{html,ts} (table of the barcodes recognized
     during processing from /api/documents/<id>/metadata/ `barcodes`). Host <pngx-document-barcodes> in the parent markup.
     Copying like CDK Clipboard: navigator.clipboard, otherwise execCommand('copy'); checkmark 3 s. -->
<script lang="ts">
  import { onDestroy } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import type { DocumentBarcode } from '../../../api/v1'
  import { isLink, copyText } from './barcodes'

  let { barcodes = [] }: { barcodes?: DocumentBarcode[] } = $props()

  let copiedIndex = $state<number | null>(null)
  let copyTimeout: ReturnType<typeof setTimeout> | undefined

  async function copy(index: number) {
    if (!(await copyText(barcodes[index].value))) return
    copiedIndex = index
    clearTimeout(copyTimeout)
    copyTimeout = setTimeout(() => (copiedIndex = null), 3000)
  }

  onDestroy(() => clearTimeout(copyTimeout))
</script>

<table class="table table-borderless align-baseline">
  <thead>
    <tr>
      <th>{t('common.page')}</th>
      <th>{t('ui.type')}</th>
      <th>{t('common.content')}</th>
      <th></th>
    </tr>
  </thead>
  <tbody>
    {#each barcodes as barcode, i (i)}
      <tr>
        <td>{barcode.page}</td>
        <td class="text-nowrap">{barcode.format}</td>
        <td class="text-break">
          {#if isLink(barcode.value)}
            <a href={barcode.value} target="_blank" rel="noopener noreferrer nofollow">{barcode.value}</a>
          {:else}
            {barcode.value}
          {/if}
        </td>
        <td class="text-end">
          <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => copy(i)} title={t('detail.barcodes.copy_content')}>
            {#if copiedIndex === i}
              <pngx-bs-icon name="clipboard-check">{@html icon('clipboard-check')}</pngx-bs-icon>
            {:else}
              <pngx-bs-icon name="clipboard">{@html icon('clipboard')}</pngx-bs-icon>
            {/if}
            <span class="visually-hidden">{t('detail.barcodes.copy_content')}</span>
          </button>
        </td>
      </tr>
    {/each}
  </tbody>
</table>
