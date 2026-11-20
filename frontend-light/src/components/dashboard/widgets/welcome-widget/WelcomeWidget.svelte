<!-- Port of dashboard/widgets/welcome-widget/welcome-widget.component.html (+ ngb-alert markup).
     Host <pngx-welcome-widget> in the parent markup. The tour (ngx-ui-tour) is not ported yet. -->
<script lang="ts">
  import '../../../../styles/components/welcome-widget.css'
  import '../../../../styles/parts/alert.css'
  import { t, tRich } from '../../../../lib/i18n'
  import BrandMark from '../../../common/logo/BrandMark.svelte'

  let { ondismiss, onstarttour }: { ondismiss: () => void; onstarttour?: () => void } = $props()

  const UPSTREAM_URL = 'https://github.com/paperless-ngx/paperless-ngx'
  const LICENSE_URL = 'https://www.gnu.org/licenses/gpl-3.0.html'
  const DOCS_URL = '/rs/docs/README.md'
  let shown = $state(true)
  const moreDetail = $derived(tRich('dashboard.more_detail_how_use'))
  const attribution = $derived(tRich('common.paperrust_based_paperless_ngx'))

  /** NgbAlert.close: fade-out transition, then (closed) */
  function close() {
    shown = false
    setTimeout(() => ondismiss(), 150)
  }
</script>

<ngb-alert class="welcome-widget bg-light shadow-sm pe-3 position-relative overflow-hidden alert fade alert-primary alert-dismissible" class:show={shown} role="alert">
  <pngx-brand-mark class="welcome-watermark d-none d-md-block position-absolute top-50 translate-middle-y text-primary pe-none pr-size-26rem"><BrandMark /></pngx-brand-mark>
  <div class="d-flex align-items-start gap-3 gap-md-4 position-relative">
    <pngx-brand-mark class="d-none d-sm-block flex-shrink-0 text-primary pr-size-3rem"><BrandMark /></pngx-brand-mark>
    <div class="flex-grow-1">
      <h4 class="alert-heading text-primary fw-semibold">{t('dashboard.paperrust_running')} 🎉</h4>
      <div class="welcome-copy text-dark">
        <p>{t("dashboard.youre_ready_start_uploading")}</p>
        <p class="mb-0">{#each moreDetail as s}{#if s.tag === 'START_LINK'}<a href={DOCS_URL} target="_blank">{s.text}</a>{:else}{s.text}{/if}{/each}</p>
      </div>
      <hr class="opacity-100">
      <div class="d-flex flex-column flex-sm-row align-items-sm-center gap-3">
        <div>
          <p class="m-0 text-secondary"><em>{t('dashboard.thanks_being_part_paperrust')}</em></p>
          <p class="m-0 small text-muted">{#each attribution as s}{#if s.tag === 'START_LINK'}<a href={UPSTREAM_URL} target="_blank" rel="noopener noreferrer">{s.text}</a>{:else if s.tag === 'START_LINK_1'}<a href={LICENSE_URL} target="_blank" rel="noopener noreferrer">{s.text}</a>{:else}{s.text}{/if}{/each}</p>
        </div>
        <button class="btn btn-primary ms-sm-auto flex-shrink-0 align-self-start align-self-sm-auto" onclick={() => onstarttour?.()}>{t('dashboard.start_tour')} &rarr;</button>
      </div>
    </div>
  </div>
  <button type="button" class="btn-close" aria-label="Close" onclick={close}></button>
</ngb-alert>
