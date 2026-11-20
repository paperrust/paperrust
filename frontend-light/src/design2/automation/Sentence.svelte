<!-- Sentence with clickable parts (topic 3 B): each part opens the matching card; "+ Condition", "+ Action" and
     "+ Else" build simple workflows right here. Sentence structure: sentence.ts (whole clauses per language). -->
<script lang="ts">
  import { t, locale } from '../../lib/i18n'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'
  import type { Summary } from './api'
  import type { CondKey, QuickAction } from './model'
  import { sentence, type Target } from './sentence'

  let {
    summary,
    active = null,
    conditions,
    quickActions,
    canElse,
    onopen,
    onaddcondition,
    onaddaction,
    onaddelse,
  }: {
    summary: Summary
    active?: Target | null
    /** Conditions offered by "+ Condition" (allowed for the first trigger) */
    conditions: { key: CondKey; label: string }[]
    quickActions: { key: QuickAction; label: string; icon: string }[]
    canElse: boolean
    onopen: (t: Target) => void
    onaddcondition: (k: CondKey) => void
    onaddaction: (k: QuickAction) => void
    onaddelse: () => void
  } = $props()

  const loc = locale()
  const s = $derived(sentence(summary, t, loc))
  const same = (a: Target | null, b?: Target) => !!a && !!b && a.card === b.card && ('index' in a ? a.index : -1) === (b && 'index' in b ? b.index : -1)
  let menu = $state<'cond' | 'act' | null>(null)
</script>

{#snippet line(segs: { text: string; target?: Target; missing?: boolean }[])}
  {#each segs as seg, i (i)}
    {#if seg.target}
      {@const tg = seg.target}
      <button type="button" class="au-part" class:missing={seg.missing} class:active={same(active, tg)} onclick={() => onopen(tg)}>{seg.text}</button>
    {:else}{seg.text}{/if}
  {/each}
{/snippet}

<div class="au-sentence" id="au-sentence">
  <p id="au-sentence-main" aria-live="polite">{@render line(s.main)}</p>
  {#if s.else}<p id="au-sentence-else">{@render line(s.else)}</p>{/if}
  <div class="au-sentence-tools">
    <div class="au-adder">
      <button type="button" class="d2-btn sm" id="au-add-condition" aria-haspopup="menu" aria-expanded={menu === 'cond'} disabled={!conditions.length} onclick={() => (menu = menu === 'cond' ? null : 'cond')}>{t('m.auto.add_condition')}</button>
      {#if menu === 'cond'}
        <Menu label={t('m.auto.add_condition')} align="left" onclose={() => (menu = null)}>
          {#each conditions as c (c.key)}
            <button type="button" class="d2-menu-item" role="menuitem" id="au-add-condition-{c.key}" onclick={() => ((menu = null), onaddcondition(c.key))}>{c.label}</button>
          {/each}
        </Menu>
      {/if}
    </div>
    <div class="au-adder">
      <button type="button" class="d2-btn sm" id="au-add-action" aria-haspopup="menu" aria-expanded={menu === 'act'} onclick={() => (menu = menu === 'act' ? null : 'act')}>{t('m.auto.add_action')}</button>
      {#if menu === 'act'}
        <Menu label={t('m.auto.add_action')} align="left" onclose={() => (menu = null)}>
          {#each quickActions as a (a.key)}
            <button type="button" class="d2-menu-item" role="menuitem" id="au-add-action-{a.key}" onclick={() => ((menu = null), onaddaction(a.key))}><Icon name={a.icon} />{a.label}</button>
          {/each}
        </Menu>
      {/if}
    </div>
    {#if canElse && !summary.else_actions.length}
      <button type="button" class="d2-btn sm ghost" id="au-add-else" onclick={onaddelse}><Icon name="signpost-split" />{t('m.auto.add_else')}</button>
    {/if}
  </div>
</div>
