<!-- Port of document-detail/document-history (Host <pngx-document-history> in the parent markup).
     Tooltip with exact time (ngbTooltip) as title attribute. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { formatDate } from '../../../lib/dates'
  import { api } from '../../../api/client'
  import type { AuditLogEntry } from '../../../api/v1'
  import { data } from '../../../stores/data.svelte'
  import { relativeDate, titleCase } from './history'
  let { documentId, dateLocale }: { documentId: number; dateLocale: string } = $props()
  let loading = $state(true)
  let entries = $state<AuditLogEntry[]>([])
  $effect(() => {
    const id = documentId
    if (!id) return
    loading = true
    api.getHistory(id).then((e) => {
      entries = e
      loading = false
    })
  })
  const tagNames = (ids: number[]) => (ids ?? []).map((id) => data.tags.get(id)?.name ?? String(id)).join(', ')
  function prettyName(key: string, id: string): string {
    const n = Number.parseInt(id, 10)
    if (!Number.isFinite(n)) return id
    switch (key) {
      case 'correspondent':
        return data.correspondents.get(n)?.name ?? id
      case 'document_type':
        return data.documentTypes.get(n)?.name ?? id
      case 'storage_path':
        return (data.storagePaths.get(n) as any)?.path ?? id
      case 'owner':
        return data.users.get(n)?.username ?? id
      default:
        return id
    }
  }
  const rel = (ts: string) => {
    const r = relativeDate(ts)
    return t(r.key).replace('%s', String(r.n))
  }
</script>

{#if loading}
    <div class="d-flex">
        <output class="spinner-border spinner-border-sm fw-normal"></output>
    </div>
{:else}
    <ul class="list-group">
        {#if entries.length === 0}
            <li class="list-group-item">
                <div class="d-flex justify-content-center">
                    <span class="fst-italic">{t('common.no_entries_found')}</span>
                </div>
            </li>
        {:else}
            {#each entries as entry (entry.id)}
                <li class="list-group-item">
                    <div class="d-flex justify-content-between align-items-center">
                        <span class="text-muted" title="{formatDate(entry.timestamp, 'longDate', dateLocale)} {formatDate(entry.timestamp, 'shortTime', dateLocale)}">{rel(entry.timestamp)}</span>
                        {#if entry.actor}
                            <span class="ms-3 fst-italic">{entry.actor.username}</span>
                        {:else}
                            <span class="ms-3 fst-italic">System</span>
                        {/if}
                        <span class="badge bg-secondary ms-auto" class:bg-primary={entry.action === 'create'}>{titleCase(entry.action)}</span>
                    </div>
                    <ul class="mt-2">
                        {#each Object.entries(entry.changes ?? {}).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)) as [key, value] (key)}
                            {#if value?.['type'] === 'm2m'}
                                <li>
                                    <span class="fst-italic">{titleCase(value['operation'])}</span>&nbsp;
                                    <span>{titleCase(key)}</span>:&nbsp;
                                    <code class="text-primary">{value['objects'].join(', ')}</code>
                                </li>
                            {:else if value?.['type'] === 'custom_field'}
                                <li>
                                    <span>{value['field']}</span>:&nbsp;
                                    <code class="text-primary">{value['value']}</code>
                                </li>
                            {:else}
                                <li>
                                    <span>{titleCase(key)}</span>:&nbsp;
                                    {#if key === 'content'}
                                        <code class="text-primary">{value[1]?.substring(0, 100)}...</code>
                                    {:else if key === 'tags'}
                                        <code class="text-primary">{tagNames(value[1])}</code>
                                    {:else}
                                        <code class="text-primary">{prettyName(key, value[1])}</code>
                                    {/if}
                                </li>
                            {/if}
                        {/each}
                    </ul>
                </li>
            {/each}
        {/if}
    </ul>
{/if}
