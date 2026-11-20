<!-- Port of document-asn (DocumentAsnComponent): route asn/:id looks up the document with this ASN
     (DocumentService.listAllFilteredIds([FILTER_ASN])) and opens it, otherwise 404. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { findDocumentIdsByAsn, asnTarget } from './document-asn'

  const asn = $derived(router.path.split('/')[1] ?? '')

  $effect(() => {
    const current = asn
    let cancelled = false
    findDocumentIdsByAsn(current)
      .catch(() => [] as number[])
      .then((ids) => {
        // Deviation: the hit also replaces the history entry (otherwise "Back" leads here again and immediately forward)
        if (!cancelled) router.navigate(asnTarget(ids), true)
      })
    return () => (cancelled = true)
  })
</script>

<p>{t('app.document_asn.searching', { INTERPOLATION: asn })}</p>
