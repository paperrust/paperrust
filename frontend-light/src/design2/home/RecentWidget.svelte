<!-- "Recently added" component: recently added documents. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import Widget from './Widget.svelte'
  import DocRows from './DocRows.svelte'
</script>

<Widget id="recent" title={t('m.home.recent')} icon="clock-history" href={router.href('documents')} linkLabel={t('common.show_all')}>
  <DocRows query={{}} sort="added" empty={t('common.no_documents_found')} />
</Widget>
