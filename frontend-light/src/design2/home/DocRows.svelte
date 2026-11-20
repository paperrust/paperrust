<!-- Short document list of a component (inbox, recently added, saved view): v1 list with
     the same permissions as the document list. Click opens the document in the workspace. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { queryString } from '../../api/v1'
  import { router } from '../../router.svelte'
  import { data } from '../../stores/data.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'

  interface Row {
    id: number
    title: string
    correspondent: number | null
    created: string
    added?: string
  }
  let {
    query,
    limit = 6,
    sort = 'created',
    empty,
    oncount = undefined,
  }: { query: Record<string, string | number | boolean | null | undefined>; limit?: number; sort?: string; empty: string; oncount?: (n: number) => void } = $props()

  let rows = $state<Row[] | null>(null)
  let failed = $state(false)
  const dateFmt = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
  const fmt = (d: string) => {
    const x = new Date(d)
    return Number.isNaN(x.getTime()) ? '' : dateFmt.format(x)
  }

  async function load() {
    try {
      const r = await api.get<{ count: number; results: Row[] }>(
        `/api/documents/${queryString({ page: 1, page_size: limit, ordering: '-' + sort, fields: 'id,title,correspondent,created,added', truncate_content: true, ...query })}`,
      )
      rows = r.results
      failed = false
      oncount?.(r.count)
    } catch {
      failed = true
      rows = []
    }
  }
  $effect(() => {
    void JSON.stringify(query)
    load()
  })
  $effect(() => consumerStatus.onDocumentConsumptionFinished(() => load()))
</script>

{#if rows === null}
  <p class="d2-muted">{t('common.loading')}</p>
{:else if failed}
  <p class="d2-muted" role="alert">{t('common.error')}</p>
{:else if rows.length === 0}
  <p class="d2-muted">{empty}</p>
{:else}
  <ul class="rows">
    {#each rows as r (r.id)}
      {@const c = r.correspondent != null ? data.correspondents.get(r.correspondent)?.name : null}
      <li>
        <a href={router.href('documents/' + r.id)} title={r.title}>
          <span class="ttl">{r.title}</span>
          <span class="meta d2-muted">{#if c}<span class="c">{c}</span>{/if}<span>{fmt(sort === 'added' ? (r.added ?? r.created) : r.created)}</span></span>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .rows {
    margin: calc(-1 * var(--d2-s2)) 0;
    padding: 0;
    list-style: none;
  }
  li + li {
    border-top: 1px solid var(--d2-border);
  }
  a {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--d2-s2) 0;
    color: var(--d2-text);
    text-decoration: none;
  }
  a:hover .ttl {
    color: var(--d2-accent-text);
    text-decoration: underline;
  }
  .ttl {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .meta {
    display: flex;
    gap: var(--d2-s2);
    font-size: var(--d2-fs-xs);
  }
  .c {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
