<!-- Frame of a home page component: heading with icon, optional "Show all" link and controls. -->
<script lang="ts">
  import type { Snippet } from 'svelte'
  import Icon from '../components/Icon.svelte'
  let {
    id,
    title,
    icon,
    href = undefined,
    linkLabel = undefined,
    tools = undefined,
    children,
  }: { id: string; title: string; icon: string; href?: string; linkLabel?: string; tools?: Snippet; children: Snippet } = $props()
</script>

<section class="w" id="d2-w-{id}" aria-labelledby="d2-w-{id}-h">
  <header class="wh">
    <h2 id="d2-w-{id}-h"><Icon name={icon} />{title}</h2>
    <span class="grow"></span>
    {#if tools}{@render tools()}{/if}
    {#if href && linkLabel}<a class="d2-btn ghost sm" {href}>{linkLabel}</a>{/if}
  </header>
  <div class="wb">{@render children()}</div>
</section>

<style>
  .w {
    display: flex;
    flex-direction: column;
    min-width: 0;
    height: 100%;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  .wh {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--d2-s2);
    padding: var(--d2-s2) var(--d2-s3);
    border-bottom: 1px solid var(--d2-border);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    margin: 0;
    font-size: var(--d2-fs-md);
  }
  h2 :global(.d2-ico) {
    color: var(--d2-text-2);
  }
  .grow {
    flex: 1;
  }
  .wb {
    flex: 1;
    min-height: 0;
    padding: var(--d2-s3);
  }
</style>
