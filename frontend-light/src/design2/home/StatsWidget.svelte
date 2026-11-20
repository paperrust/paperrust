<!-- "Statistics" component: key figures from GET /api/statistics/ (like the classic dashboard, without file type chart). -->
<script lang="ts">
  import { t, locale } from '../../lib/i18n'
  import { api } from '../../api/client'
  import { router } from '../../router.svelte'
  import { consumerStatus } from '../../stores/uploads.svelte'
  import Widget from './Widget.svelte'

  interface Stats {
    documents_total?: number
    documents_inbox?: number | null
    character_count?: number
    tag_count?: number
    correspondent_count?: number
    document_type_count?: number
    storage_path_count?: number
    current_asn?: number | null
  }
  let s = $state<Stats | null>(null)
  let failed = $state(false)
  const nf = $derived(new Intl.NumberFormat(locale()))
  async function load() {
    try {
      s = await api.get<Stats>('/api/statistics/')
      failed = false
    } catch {
      failed = true
    }
  }
  $effect(() => {
    load()
    return consumerStatus.onDocumentConsumptionFinished(() => load())
  })
  const rows = $derived(
    s
      ? ([
          [t('dashboard.total_documents'), s.documents_total, 'documents'],
          [t('dashboard.documents_in_inbox'), s.documents_inbox, 'inbox'],
          [t('common.tags'), s.tag_count, 'attributes/tags'],
          [t('common.correspondents'), s.correspondent_count, 'attributes/correspondents'],
          [t('common.document_types'), s.document_type_count, 'attributes/documenttypes'],
          [t('common.storage_paths'), s.storage_path_count, 'attributes/storagepaths'],
          [t('dashboard.total_characters'), s.character_count, null],
          [t('dashboard.current_asn'), s.current_asn, null],
        ] as [string, number | null | undefined, string | null][]).filter(([, v]) => v != null)
      : [],
  )
</script>

<Widget id="stats" title={t('dashboard.statistics')} icon="grid-1x2">
  {#if failed}
    <p class="d2-muted" role="alert">{t('common.error')}</p>
  {:else if !s}
    <p class="d2-muted">{t('common.loading')}</p>
  {:else}
    <dl class="kv">
      {#each rows as [label, value, href] (label)}
        <div>
          <dt class="d2-muted">{label}</dt>
          <dd>{#if href}<a href={router.href(href)}>{nf.format(value ?? 0)}</a>{:else}{nf.format(value ?? 0)}{/if}</dd>
        </div>
      {/each}
    </dl>
  {/if}
</Widget>

<style>
  .kv {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: var(--d2-s3);
    margin: 0;
  }
  dt {
    font-size: var(--d2-fs-xs);
  }
  dd {
    margin: 0;
    font-size: var(--d2-fs-lg, 1.25rem);
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  dd a {
    color: var(--d2-text);
    text-decoration: none;
  }
  dd a:hover {
    color: var(--d2-accent-text);
    text-decoration: underline;
  }
</style>
