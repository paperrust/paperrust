<!-- "Inbox" component: newest documents with inbox tag. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import Widget from './Widget.svelte'
  import DocRows from './DocRows.svelte'
  let count = $state<number | null>(null)
</script>

<Widget id="inbox" title={count === null ? t('common.inbox') : `${t('common.inbox')} (${count})`} icon="inbox" href={router.href('inbox')} linkLabel={t('common.show_all')}>
  <DocRows query={{ is_in_inbox: 'true' }} sort="added" empty={t('m.home.inbox_empty')} oncount={(n) => (count = n)} />
</Widget>
