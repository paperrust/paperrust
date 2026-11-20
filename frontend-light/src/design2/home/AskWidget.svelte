<!-- "Question to the archive" component (only with AI): enter question → assistant (`assistant?q=…` asks it immediately,
     same API /rs/api/v2/ask as the assistant). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import Widget from './Widget.svelte'
  import Icon from '../components/Icon.svelte'

  let q = $state('')
  function submit(e: SubmitEvent) {
    e.preventDefault()
    const v = q.trim()
    if (v) router.navigate('assistant?q=' + encodeURIComponent(v))
  }
</script>

<Widget id="ask" title={t('m.home.ask_archive')} icon="stars" href={router.href('assistant')} linkLabel={t('m.home.open_assistant')}>
  <form class="ask" onsubmit={submit} role="search">
    <label class="d2-sr" for="d2-w-ask-q">{t('m.home.ask_archive')}</label>
    <input id="d2-w-ask-q" class="d2-input" type="search" bind:value={q} placeholder={t('m.home.ask_placeholder')} autocomplete="off" />
    <button class="d2-btn" disabled={!q.trim()}><Icon name="send" />{t('m.home.ask')}</button>
  </form>
</Widget>

<style>
  .ask {
    display: flex;
    gap: var(--d2-s2);
  }
  .ask input {
    flex: 1;
    min-width: 0;
  }
</style>
