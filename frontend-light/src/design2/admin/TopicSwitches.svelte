<!-- Fine-tuning: topics × view / edit / delete as switches (role="switch", "partial" = aria-checked
     mixed). Each topic can be expanded: the same switches per model (e.g. tasks view only), so that roles
     like "Manage" can also be represented and changed losslessly. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import Icon from '../components/Icon.svelte'
  import { ACTIONS, modelState, permsToSwitches, type Action, type Topic } from './roles'
  import { modelName, topicHint, topicName } from './labels'

  let {
    topics,
    perms,
    onchange,
    onmodel,
    disabled = false,
    idPrefix = 'ad-sw',
  }: {
    topics: Topic[]
    perms: string[]
    onchange: (topic: string, action: Action, on: boolean) => void
    onmodel: (model: string, action: Action, on: boolean) => void
    disabled?: boolean
    idPrefix?: string
  } = $props()
  const sws = $derived(permsToSwitches(perms, topics))
  const ACT: Record<Action, string> = { view: 'm.admin.act_view', edit: 'm.admin.act_edit', delete: 'common.delete' }
  /** expanded topics; topics with partial states are expanded on open */
  let open = $state<Record<string, boolean>>(
    (() => Object.fromEntries(Object.entries(permsToSwitches(perms, topics)).map(([k, row]) => [k, ACTIONS.some((a) => row[a] === 'mixed')])))(),
  )
  const aria = (v: boolean | 'mixed') => (v === 'mixed' ? 'mixed' : v ? 'true' : 'false')
</script>

{#snippet toggle(v: boolean | 'mixed', label: string, onclick: () => void)}
  <button type="button" role="switch" class="tg" class:on={v === true} class:mixed={v === 'mixed'} aria-checked={aria(v)} aria-label="{label}{v === 'mixed' ? ` (${t('m.admin.partial')})` : ''}" {disabled} {onclick}><span class="knob"></span></button>
{/snippet}

<table class="sw">
  <thead>
    <tr>
      <th scope="col"><span class="d2-sr">{t('m.admin.topic')}</span></th>
      {#each ACTIONS as a (a)}<th scope="col">{t(ACT[a])}</th>{/each}
    </tr>
  </thead>
  {#each topics as tp (tp.key)}
    {@const key = String(tp.key)}
    {@const row = sws[key]}
    <tbody>
      <tr>
        <th scope="row">
          <button type="button" class="exp" aria-expanded={!!open[key]} aria-controls="{idPrefix}-{key}-m" onclick={() => (open[key] = !open[key])}>
            <Icon name="chevron-right" />
            <span><span class="tn">{topicName(key)}</span><span class="th">{topicHint(key)}</span></span>
          </button>
        </th>
        {#each ACTIONS as a (a)}
          <td>{@render toggle(row[a], `${topicName(key)}: ${t(ACT[a])}`, () => onchange(key, a, row[a] !== true))}</td>
        {/each}
      </tr>
    </tbody>
    <tbody id="{idPrefix}-{key}-m" class="models" hidden={!open[key]}>
      {#each tp.models as m (m)}
        {@const ms = modelState(perms, m)}
        <tr>
          <th scope="row" class="mn">{modelName(m)}</th>
          {#each ACTIONS as a (a)}
            <td>{@render toggle(ms[a], `${modelName(m)}: ${t(ACT[a])}`, () => onmodel(m, a, ms[a] !== true))}</td>
          {/each}
        </tr>
      {/each}
    </tbody>
  {/each}
</table>

<style>
  .sw {
    width: 100%;
    margin: 0;
    border-collapse: collapse;
    font-size: var(--d2-fs-sm);
  }
  th,
  td {
    padding: var(--d2-s2) var(--d2-s1);
    border-bottom: 1px solid var(--d2-border);
    text-align: center;
    vertical-align: middle;
  }
  thead th {
    color: var(--d2-text-2);
    font-size: var(--d2-fs-xs);
    font-weight: 600;
    width: 72px;
  }
  thead th:first-child {
    width: auto;
  }
  tbody th {
    text-align: left;
    font-weight: 500;
  }
  .exp {
    display: flex;
    align-items: flex-start;
    gap: var(--d2-s1);
    width: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .exp :global(.d2-ico) {
    margin-top: 3px;
    color: var(--d2-text-3);
    transition: transform 0.12s;
  }
  .exp[aria-expanded='true'] :global(.d2-ico) {
    transform: rotate(90deg);
  }
  .tn {
    display: block;
    color: var(--d2-text);
  }
  .th {
    display: block;
    color: var(--d2-text-3);
    font-size: var(--d2-fs-xs);
    font-weight: 400;
  }
  .models th,
  .models td {
    padding-top: 4px;
    padding-bottom: 4px;
    background: var(--d2-surface-2);
  }
  .mn {
    padding-left: 22px;
    color: var(--d2-text-2);
    font-weight: 400;
  }
  .tg {
    position: relative;
    width: 36px;
    height: 20px;
    padding: 0;
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r-pill);
    background: var(--d2-surface-3);
    cursor: pointer;
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--d2-surface);
    box-shadow: var(--d2-shadow-1);
    transition: left 0.12s;
  }
  .tg.on {
    background: var(--d2-accent);
    border-color: var(--d2-accent);
  }
  .tg.on .knob {
    left: 18px;
  }
  .tg.mixed {
    background: repeating-linear-gradient(135deg, var(--d2-surface-3) 0 4px, var(--d2-border-strong) 4px 6px);
  }
  .tg.mixed .knob {
    left: 10px;
  }
  .tg:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
