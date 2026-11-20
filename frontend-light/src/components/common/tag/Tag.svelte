<!-- Port of common/tag/tag.component.html. Host <pngx-tag> in the parent markup. -->
<script lang="ts">
  import '../../../styles/components/tag.css'
  import { t } from '../../../lib/i18n'
  import type { Tag } from '../../../api/v1'
  let {
    tag,
    loading = false,
    clickable = false,
    linkTitle = '',
    onclick,
  }: { tag?: Tag | null; loading?: boolean; clickable?: boolean; linkTitle?: string; onclick?: (e: MouseEvent) => void } = $props()
</script>

{#if tag}
  {#if !clickable}
    <span class="badge" style:background={tag.color} style:color={tag.text_color}>{tag.name}</span>
  {/if}
  {#if clickable}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions, a11y_missing_attribute -->
    <a title={linkTitle} class="badge" style:background={tag.color} style:color={tag.text_color} {onclick}>{tag.name}</a>
  {/if}
{:else if loading}
  <span class="placeholder-glow">
    <span class="placeholder badge private">
      <span class="text-dark">Loading...</span>
    </span>
  </span>
{:else}
  {#if !clickable}
    <span class="badge private">{t('common.private')}</span>
  {/if}
  {#if clickable}
    <!-- svelte-ignore a11y_missing_attribute -->
    <a title={linkTitle} class="badge private">{t('common.private')}</a>
  {/if}
{/if}
