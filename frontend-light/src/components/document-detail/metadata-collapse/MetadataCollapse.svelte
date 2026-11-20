<!-- Port of document-detail/metadata-collapse (Host <pngx-metadata-collapse> in the parent markup). ngbCollapse → .collapse/.show -->
<script lang="ts">
  import '../../../styles/components/metadata-collapse.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import type { MetadataEntry } from '../../../api/v1'
  let { title, metadata = [] }: { title: string; metadata?: MetadataEntry[] } = $props()
  let expand = $state(false)
</script>

<h6>
  <button type="button" class="btn btn-outline-secondary btn-sm me-2"
    onclick={() => (expand = !expand)} aria-label={t('detail.toggle_document_metadata')}>
    {#if !expand}
      <pngx-bs-icon width="1.2em" height="1.2em" name="caret-down">{@html icon('caret-down', '1.2em', '1.2em')}</pngx-bs-icon>
    {/if}
    {#if expand}
      <pngx-bs-icon width="1.2em" height="1.2em" name="caret-up">{@html icon('caret-up', '1.2em', '1.2em')}</pngx-bs-icon>
    {/if}
  </button>
  {title}
</h6>

<div class="collapse" class:show={expand}>
  <table class="table table-borderless">
    <tbody>
      {#each metadata as m}
        <tr>
          <td>{m.prefix}:{m.key}</td>
          <td class="metadata-column">{m.value}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
