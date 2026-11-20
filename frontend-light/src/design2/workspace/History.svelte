<!-- History (audit log, v1 /api/documents/{id}/history/). -->
<script lang="ts">
  import { t, msg } from '../../lib/i18n'
  import { api } from '../../api/client'
  import type { AuditLogEntry } from '../../api/v1'
  import { data } from '../../stores/data.svelte'

  let { id }: { id: number } = $props()
  let entries = $state<AuditLogEntry[] | null>(null)
  let error = $state('')
  const fmt = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })
  $effect(() => {
    const my = id
    entries = null
    error = ''
    api
      .getHistory(my)
      .then((r) => my === id && (entries = r))
      .catch((e) => (error = e?.status === 403 ? t('m.workspace.not_permission_view_history') : String(e?.message ?? e)))
  })
  const ACTIONS: Record<string, string> = { create: msg('common.created'), update: msg('m.workspace.changed'), delete: msg('m.workspace.deleted') }
  function value(field: string, v: unknown): string {
    if (v == null || v === 'None') return '–'
    const n = Number(v)
    if (field === 'correspondent') return data.correspondents.get(n)?.name ?? String(v)
    if (field === 'document_type') return data.documentTypes.get(n)?.name ?? String(v)
    if (field === 'storage_path') return data.storagePaths.get(n)?.name ?? String(v)
    if (typeof v === 'object') return JSON.stringify(v)
    return String(v)
  }
  function changes(e: AuditLogEntry): [string, string][] {
    const out: [string, string][] = []
    for (const [k, v] of Object.entries(e.changes ?? {})) {
      if (Array.isArray(v) && v.length === 2) out.push([k, `${value(k, v[0])} → ${value(k, v[1])}`])
      else if (v && typeof v === 'object' && 'objects' in (v as any)) out.push([k, `${(v as any).operation ?? ''} ${((v as any).objects ?? []).join(', ')}`])
      else out.push([k, value(k, v)])
    }
    return out
  }
</script>

<div class="hist">
  {#if error}
    <p class="d2-muted">{error}</p>
  {:else if entries === null}
    <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
  {:else if entries.length === 0}
    <p class="d2-muted">{t('common.no_entries_found')}</p>
  {:else}
    <ol>
      {#each entries as e (e.id)}
        <li>
          <div class="hd"><strong>{t(ACTIONS[e.action] ?? e.action)}</strong><span class="d2-muted">{fmt.format(new Date(e.timestamp))}{#if e.actor} · {e.actor.username}{/if}</span></div>
          {#each changes(e) as [k, v] (k)}
            <div class="ch"><span class="k">{k}</span><span class="v">{v}</span></div>
          {/each}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .hist {
    padding: var(--d2-s4);
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0 0 0 var(--d2-s4);
    border-left: 2px solid var(--d2-border);
  }
  li {
    position: relative;
    padding-bottom: var(--d2-s4);
  }
  li::before {
    content: '';
    position: absolute;
    left: calc(-1 * var(--d2-s4) - 6px);
    top: 5px;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--d2-accent);
  }
  .hd {
    display: flex;
    flex-wrap: wrap;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
  .ch {
    display: grid;
    grid-template-columns: 120px 1fr;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-sm);
  }
  .k {
    color: var(--d2-text-3);
  }
  .v {
    word-break: break-word;
  }
</style>
