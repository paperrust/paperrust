<!-- Banner in read-only mode (ransomware protection, PaperRust extension). Sits above every page
     (included in AppFrame). Queries /api/x-guard/status/ at start and every minute; superusers
     get the way to the check (route protection). -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { router } from '../../../router.svelte'
  import { session } from '../../../stores/settings.svelte'
  import { guardStatus, type GuardStatus } from './guard-api'

  let status = $state<GuardStatus | null>(null)
  const isSuperuser = $derived(!!session.data?.user?.is_superuser)

  async function load() {
    try {
      status = await guardStatus()
    } catch {
      // older servers without /api/x-guard or logged out: no banner
    }
  }

  onMount(() => {
    load()
    const id = setInterval(load, 60_000)
    // reload immediately after lifting on the protection page
    const onChange = () => load()
    window.addEventListener('paperrust:guard-changed', onChange)
    return () => {
      clearInterval(id)
      window.removeEventListener('paperrust:guard-changed', onChange)
    }
  })
</script>

{#if status?.read_only}
  <div class="alert alert-danger d-flex flex-wrap align-items-center gap-2 mt-3 mb-2" role="alert">
    <pngx-bs-icon name="shield-exclamation" class="me-1">{@html icon('shield-exclamation')}</pngx-bs-icon>
    <div class="me-auto">
      <strong>{t('protection.banner_title')}</strong>
      <span class="ms-1">{t('protection.banner_text')}</span>
      {#if status.since}<span class="small ms-1">({new Date(status.since).toLocaleString()})</span>{/if}
    </div>
    {#if isSuperuser && router.path !== 'protection'}
      <a class="btn btn-sm btn-light" href={router.href('protection')}>{t('protection.banner_review')}</a>
    {/if}
  </div>
{/if}
