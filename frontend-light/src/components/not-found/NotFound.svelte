<!-- Port of not-found (NotFoundComponent): 404 page, also catch-all route for unknown paths. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { router } from '../../router.svelte'
  import Logo from '../common/logo/Logo.svelte'
</script>

<div class="d-sm-flex flex-wrap p-2 align-items-center justify-content-center" style={"height: 75vh;"}>
    <div class="p-4 pt-0">
      <h1 style={"font-size: 10rem;"} class="fw-bold">404</h1>
    </div>
    <div class="p-4 pt-0">
      <h1 class="display-6">{t('app.not_found.title')}</h1>
      <p>
        <a class="btn btn-primary" href={router.href('dashboard')}>
          <pngx-bs-icon width="1.2em" height="1.2em" name="house" class="me-1">{@html icon('house', '1.2em', '1.2em')}</pngx-bs-icon>{t('app.not_found.go_dashboard')}
        </a>
      </p>
    </div>
    <pngx-logo extra_classes="p-3 position-absolute bottom-0 start-50 translate-middle"><Logo extraClasses="p-3 position-absolute bottom-0 start-50 translate-middle" /></pngx-logo>
  </div>
