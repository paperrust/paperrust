<!-- Port of admin/config/config.component.html (route config). Host <pngx-config> in App.svelte. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { docsUrl } from '../../../lib/docs-url'
  import { toastService } from '../../../stores/toasts.svelte'
  import { session } from '../../../stores/settings.svelte'
  import { api } from '../../../api/client'
  import { uploadForm } from '../../../api/admin'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import InputSelect from '../../common/input/select/InputSelect.svelte'
  import InputNumber from '../../common/input/number/InputNumber.svelte'
  import InputSwitch from '../../common/input/switch/InputSwitch.svelte'
  import InputText from '../../common/input/text/InputText.svelte'
  import InputFile from '../../common/input/file/InputFile.svelte'
  import InputPassword from '../../common/input/password/InputPassword.svelte'
  import { ConfigCategory, ConfigOptionType, PaperlessConfigOptions } from './config-options'
  import { categorySections, categoryOptions, configToForm, formToSave, isConfigDirty } from './config'

  const optionCategories = Object.values(ConfigCategory)
  let active = $state(optionCategories[0])
  let loading = $state(true)
  let form = $state<Record<string, any>>({})
  let store = $state<Record<string, any>>({})
  let initial: Record<string, any> = {}
  let externallyConfigured = $state(new Set<string>())
  const dirty = $derived(isConfigDirty(form, store))
  /** Validate JSON fields (validator) */
  const errors = $derived.by(() => {
    const e: Record<string, string | null> = {}
    for (const o of PaperlessConfigOptions.filter((o) => o.type === ConfigOptionType.JSON)) {
      const v = form[o.key]
      if (!v || v.toString().length === 0) continue
      try {
        JSON.parse(v)
      } catch {
        e[o.key] = t('common.invalid_json')
      }
    }
    return { ...serverErrors, ...e }
  })
  let serverErrors = $state<Record<string, any>>({})
  const valid = $derived(!Object.values(errors).some(Boolean))

  function initialize(config: Record<string, any>) {
    externallyConfigured = new Set(config.externally_configured_variables ?? [])
    const f = configToForm(config)
    store = f
    form = { ...f }
    initial = config
  }

  api
    .get<Record<string, any>[]>('/api/config/')
    .then((configs) => {
      loading = false
      initialize(configs[0])
    })
    .catch((e) => {
      loading = false
      toastService.showError(t('admin.config.error_retrieving_config'), e)
    })

  /** SettingsService.initializeSettings: reload app title/logo */
  async function refreshUiSettings() {
    try {
      session.data = await api.uiSettings()
    } catch {}
  }

  async function saveConfig(e: Event) {
    e.preventDefault()
    loading = true
    try {
      const config = await api.request<Record<string, any>>('PATCH', `/api/config/${form.id}/`, formToSave(form))
      loading = false
      initialize(config)
      refreshUiSettings()
      toastService.showSuccess(t('admin.config.configuration_updated'))
    } catch (err) {
      loading = false
      toastService.showError(t('admin.config.error_occurred_updating_configuration'), err)
    }
  }

  function discardChanges() {
    form = configToForm(initial)
  }

  async function uploadFile(file: File, key: string) {
    loading = true
    try {
      const fd = new FormData()
      fd.append(key, file, file.name)
      const config = await uploadForm<Record<string, any>>('PATCH', `/api/config/${form.id}/`, fd)
      loading = false
      initialize(config)
      refreshUiSettings()
      toastService.showSuccess(t('admin.config.file_successfully_updated'))
    } catch (err) {
      loading = false
      toastService.showError(t('admin.config.error_occurred_uploading_file'), err)
    }
  }

  const isSet = (key: string) => form[key] != null
  // The server lists each setting once under its upstream name (PAPERLESS_X, like paperless-ngx);
  // config_key shows the PaperRust name PAPERRUST_X of the same setting.
  const isExternal = (k?: string) =>
    !!k && (externallyConfigured.has(k) || externallyConfigured.has(k.replace(/^PAPERRUST_/, 'PAPERLESS_')))
  function navClick(e: Event, c: string) {
    e.preventDefault()
    active = c
  }
</script>

<pngx-page-header>
<PageHeader
    title={t('admin.config.application_configuration')}
    info={t('admin.config.global_app_configuration_options')}
    infoLink="configuration">
</PageHeader>
</pngx-page-header>

<form onsubmit={saveConfig} class="pb-4" autocomplete="off">

    <ul class="nav-tabs nav" role="tablist">
        {#each optionCategories as category, i (category)}
            <li class="nav-item" role="presentation">
                <a href="" class="nav-link" class:active={active === category} role="tab" id="ngb-nav-{i}" aria-controls={active === category ? `ngb-nav-${i}-panel` : null} aria-selected={active === category} aria-disabled="false" onclick={(e) => navClick(e, category)}>{t(category)}</a>
            </li>
        {/each}
    </ul>
    <div class="border-start border-end border-bottom p-3 mb-3 shadow-sm">
        <div class="tab-pane fade show active" role="tabpanel" id="ngb-nav-{optionCategories.indexOf(active)}-panel" aria-labelledby="ngb-nav-{optionCategories.indexOf(active)}">
            <div class="p-3">
                {#each categorySections(active) as section (section)}
                    {#if section}
                        <h5 class="mt-4 mb-3">{t(section)}</h5>
                    {/if}
                    <div class="row row-cols-1 row-cols-md-2 row-cols-lg-3 g-2">
                        {#each categoryOptions(active, section) as option (option.key)}
                            <div class="col">
                                <div class="card bg-light">
                                    <div class="card-body">
                                        <div class="card-title d-flex align-items-center flex-wrap">
                                            <h6 class="mb-0">
                                              {t(option.title)}
                                            </h6>
                                            <a class="btn btn-sm btn-link" title={t('admin.config.read_documentation_about_setting')} href={docsUrl(`configuration/#${option.config_key}`)} target="_blank" referrerpolicy="no-referrer">
                                                <pngx-bs-icon name="info-circle">{@html icon('info-circle')}</pngx-bs-icon>
                                            </a>
                                            {#if isExternal(option.config_key)}
                                              {#if isSet(option.key)}
                                                <span class="badge rounded-pill bg-body-secondary text-dark fw-normal" title={t('admin.config.value_overrides_set_outside', { INTERPOLATION: option.config_key })}>{t('admin.config.overrides_external')}</span>
                                              {:else}
                                                <span class="badge rounded-pill bg-body-secondary text-dark fw-normal" title={t('admin.config.set_outside_paperrust_enter', { INTERPOLATION: option.config_key })}>{t('admin.config.set_externally')}</span>
                                              {/if}
                                            {/if}
                                            {#if isSet(option.key)}
                                              {#if isExternal(option.config_key)}
                                                <button type="button" class="btn btn-sm btn-link text-danger ms-auto pe-0" title={t('admin.config.use_externally_configured_value')} onclick={() => (form[option.key] = null)}>
                                                    <pngx-bs-icon class="me-1" name="x">{@html icon('x')}</pngx-bs-icon>{t('admin.config.reset_external')}
                                                </button>
                                              {:else}
                                                <button type="button" class="btn btn-sm btn-link text-danger ms-auto pe-0" title={t('common.reset')} onclick={() => (form[option.key] = null)}>
                                                    <pngx-bs-icon class="me-1" name="x">{@html icon('x')}</pngx-bs-icon>{t('common.reset')}
                                                </button>
                                              {/if}
                                            {/if}
                                        </div>
                                        <div class="mb-n3">
                                            {#if option.type === ConfigOptionType.Select}<pngx-input-select><InputSelect bind:value={form[option.key]} error={errors[option.key]} items={option.choices} allowNull={true} /></pngx-input-select>
                                            {:else if option.type === ConfigOptionType.Number}<pngx-input-number><InputNumber bind:value={form[option.key]} error={errors[option.key]} showAdd={false} /></pngx-input-number>
                                            {:else if option.type === ConfigOptionType.Boolean}<pngx-input-switch><InputSwitch value={form[option.key]} onchange={(v) => (form[option.key] = v)} showUnsetNote={true} horizontal={true} title={t('admin.config.enable')} /></pngx-input-switch>
                                            {:else if option.type === ConfigOptionType.String || option.type === ConfigOptionType.JSON}<pngx-input-text><InputText bind:value={form[option.key]} error={errors[option.key]} /></pngx-input-text>
                                            {:else if option.type === ConfigOptionType.File}<pngx-input-file><InputFile bind:value={form[option.key]} onupload={(f) => uploadFile(f, option.key)} error={errors[option.key]} /></pngx-input-file>
                                            {:else if option.type === ConfigOptionType.Password}<pngx-input-password><InputPassword bind:value={form[option.key]} error={errors[option.key]} autocomplete="new-password" /></pngx-input-password>
                                            {/if}
                                        </div>
                                        {#if option.note}
                                            <div class="form-text fst-italic">{t(option.note)}</div>
                                        {/if}
                                    </div>
                                </div>
                            </div>
                        {/each}
                    </div>
                {/each}
            </div>
        </div>
    </div>
    <div class="btn-toolbar" role="toolbar">
        <div class="btn-group me-2">
            <button type="button" onclick={discardChanges} class="btn btn-outline-secondary" disabled={loading || !dirty}>{t('common.cancel')}</button>
        </div>
        <div class="btn-group">
            <button type="submit" class="btn btn-primary" disabled={loading || !valid || !dirty}>{t('common.save')}</button>
        </div>
    </div>
</form>
