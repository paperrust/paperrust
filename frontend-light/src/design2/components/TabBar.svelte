<!-- Bottom tab bar on the phone (< 768 px): home, inbox, documents, more (drawer with views,
     administration, settings). No assistant – questions via the search at the top (docs/DESIGN2-ENTWIRREN.md §5–6). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { ui } from '../stores/ui.svelte'
  import Icon from './Icon.svelte'
  const tabs = $derived([
    { p: 'dashboard', icon: 'grid-1x2', label: t('common.dashboard'), on: router.path === 'dashboard' },
    { p: 'inbox', icon: 'inbox', label: t('common.inbox'), on: router.path === 'inbox' },
    { p: 'documents', icon: 'files', label: t('common.documents_2'), on: /^(documents|view)/.test(router.path) },
  ])
</script>

<nav class="tabs" aria-label={t('m.ui.sections')}>
  {#each tabs as x (x.p)}
    <a href={router.href(x.p)} class:on={x.on} aria-current={x.on ? 'page' : undefined}><Icon name={x.icon} size="1.2rem" /><span>{x.label}</span></a>
  {/each}
  <button onclick={() => (ui.navOpen = !ui.navOpen)} aria-expanded={ui.navOpen}><Icon name="three-dots" size="1.2rem" /><span>{t('m.ui.more')}</span></button>
</nav>

<style>
  .tabs {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    background: var(--d2-surface);
    border-top: 1px solid var(--d2-border);
    padding-bottom: env(safe-area-inset-bottom);
  }
  a,
  button {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    border: 0;
    background: none;
    color: var(--d2-text-2);
    font-size: 11px;
    font-weight: 500;
    text-decoration: none;
    cursor: pointer;
  }
  span {
    max-width: 100%;
    padding: 0 2px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .on {
    color: var(--d2-accent-text);
  }
</style>
