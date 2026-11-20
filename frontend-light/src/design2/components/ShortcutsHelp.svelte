<!-- Overview of the keyboard shortcuts (?). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { KEYMAP } from '../lib/keys'
  import { ui } from '../stores/ui.svelte'
  import SideSheet from './SideSheet.svelte'
</script>

<SideSheet title={t('common.keyboard_shortcuts')} onclose={() => (ui.helpOpen = false)} width="420px">
  <dl class="keys">
    {#each KEYMAP as k (k.label)}
      <dt>{#each k.keys as key, i (i)}{#if i > 0}<span class="plus">{k.keys[0] === 'g' ? ' ' : '+'}</span>{/if}<kbd class="d2-kbd">{key}</kbd>{/each}</dt>
      <dd>{t(k.label)}</dd>
    {/each}
  </dl>
</SideSheet>

<style>
  .keys {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: var(--d2-s3) var(--d2-s4);
    margin: 0;
    align-items: center;
  }
  dt {
    white-space: nowrap;
  }
  dd {
    margin: 0;
    color: var(--d2-text-2);
  }
  .plus {
    padding: 0 3px;
    color: var(--d2-text-3);
  }
</style>
