<!-- Role selection: radio buttons with a one-sentence explanation. "Individual" appears only as a state (not selectable),
     "Via groups only" (no own permissions) only for users. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import type { Role } from './roles'
  import { roleDesc, roleName } from './labels'

  let {
    roles,
    value,
    onpick,
    allowNone = false,
    individual = false,
    disabled = false,
    name = 'ad-role',
    labelledby,
  }: {
    roles: Role[]
    /** selected role (id), 'none' or null (= Individual) */
    value: number | string | null
    onpick: (v: number | string) => void
    allowNone?: boolean
    individual?: boolean
    disabled?: boolean
    name?: string
    labelledby: string
  } = $props()
</script>

<div class="rp" role="radiogroup" aria-labelledby={labelledby}>
  {#each roles as r (r.id)}
    <label class="opt" class:on={!individual && String(value) === String(r.id)}>
      <input type="radio" {name} value={String(r.id)} checked={!individual && String(value) === String(r.id)} {disabled} onchange={() => onpick(r.id)} />
      <span class="txt"><span class="n">{roleName(r)}</span><span class="d">{roleDesc(r)}</span></span>
    </label>
  {/each}
  {#if allowNone}
    <label class="opt" class:on={!individual && value === 'none'}>
      <input type="radio" {name} value="none" checked={!individual && value === 'none'} {disabled} onchange={() => onpick('none')} />
      <span class="txt"><span class="n">{t('m.admin.role_via_groups')}</span><span class="d">{t('m.admin.role_none_desc')}</span></span>
    </label>
  {/if}
  {#if individual}
    <div class="opt on ind" aria-live="polite">
      <span class="dot" aria-hidden="true"></span>
      <span class="txt"><span class="n">{t('m.admin.role_individual')}</span><span class="d">{t('m.admin.role_individual_desc')}</span></span>
    </div>
  {/if}
</div>

<style>
  .rp {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s1);
  }
  .opt {
    display: flex;
    align-items: flex-start;
    gap: var(--d2-s2);
    padding: var(--d2-s2) var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
    cursor: pointer;
  }
  .opt:hover {
    background: var(--d2-surface-2);
  }
  .opt.on {
    border-color: var(--d2-accent);
    background: var(--d2-accent-soft);
  }
  .opt.ind {
    cursor: default;
  }
  input {
    margin: 3px 0 0;
    width: 16px;
    height: 16px;
    flex: none;
    accent-color: var(--d2-accent);
  }
  .dot {
    width: 16px;
    height: 16px;
    margin-top: 3px;
    flex: none;
    border: 5px solid var(--d2-accent);
    border-radius: 50%;
  }
  .txt {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .n {
    font-weight: 600;
    color: var(--d2-text);
  }
  .d {
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
  }
</style>
