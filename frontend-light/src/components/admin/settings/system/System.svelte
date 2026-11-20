<!-- "System" tab of the settings (docs/SETTINGS.md §4.1, WP6): system settings via
     /rs/api/v2/settings. Host <pngx-settings-system> in App.svelte, route settings/system[/<group>].
     Own UI without Angular counterpart; Bootstrap building blocks like the other settings tabs. -->
<script lang="ts">
  import { untrack } from 'svelte'
  import { LLM_LANGUAGE_CHOICES } from '../../config/config-options'
  import { t } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { docsUrl } from '../../../../lib/docs-url'
  import { router, BASE as APP_BASE } from '../../../../router.svelte'
  import { toastService } from '../../../../stores/toasts.svelte'
  import PageHeader from '../../../common/page-header/PageHeader.svelte'
  import * as sapi from './api'
  import type { HistoryRow, OriginItem, SettingItem, SettingsList } from './api'
  import { GROUPS, collectChanges, cronText, displayKey, displayName, editable, fileName, formerNames, groupOf, helpText, matches, toForm, type Filter } from './system'

  const SETTINGS_TABS: [string, string][] = [
    ['general', t('common.general')],
    ['documents', t('common.documents_2')],
    ['permissions', t('common.permissions')],
    ['notifications', t('common.notifications')],
  ]

  const group = $derived.by(() => {
    const g = (router.path.split('/')[2] ?? '').toLowerCase()
    return GROUPS.some((x) => x.id === g) ? g : 'general'
  })
  let view = $state<'settings' | 'origin' | 'history'>('settings')
  let data = $state<SettingsList | null>(null)
  let loadError = $state<string | null>(null)
  let form = $state<Record<string, string | boolean>>({})
  let initial = $state<Record<string, string | boolean>>({})
  let errors = $state<Record<string, string>>({})
  let saving = $state(false)
  let filter = $state<Filter>({ q: '', changed: false, restart: false, readOnly: false })

  const items = $derived((data?.items ?? []).filter((i) => matches(i, filter)))
  const searching = $derived(filter.q.trim() !== '' || filter.changed || filter.restart)
  const shown = $derived(searching ? items : items.filter((i) => groupOf(i) === group))
  const counts = $derived.by(() => {
    const c: Record<string, number> = {}
    for (const i of items) c[groupOf(i)] = (c[groupOf(i)] ?? 0) + 1
    return c
  })
  const dirtyKeys = $derived(Object.keys(form).filter((k) => form[k] !== initial[k]))

  async function load() {
    try {
      const d = await sapi.listSettings()
      const f: Record<string, string | boolean> = {}
      for (const i of d.items) f[i.key] = toForm(i)
      data = d
      initial = f
      form = { ...f }
      errors = {}
      loadError = null
    } catch (e: any) {
      loadError = e?.status === 403 ? t('admin.settings.not_permission_change_system') : sapi.v2Error(e).message
    }
  }
  untrack(() => load())

  function goGroup(e: Event, id: string) {
    e.preventDefault()
    view = 'settings'
    router.navigate(`settings/system/${id}`)
  }
  function goSettings(e: Event, key: string) {
    e.preventDefault()
    router.navigate(`settings/${key}`)
  }

  async function save() {
    if (!data) return
    const r = collectChanges(shown, form, initial)
    errors = r.errors
    if (Object.keys(r.errors).length || !Object.keys(r.values).length) return
    saving = true
    try {
      const desktop = Object.keys(r.values).filter((k) => k.startsWith('desktop.'))
      const other = Object.fromEntries(Object.entries(r.values).filter(([k]) => !k.startsWith('desktop.')))
      if (Object.keys(other).length) {
        const res = await sapi.patchSettings(other, data.rev)
        toastService.show({
          content: res.reindex_suggested ? t('admin.settings.settings_saved_please_rebuild') : t('admin.settings.settings_saved_successfully'),
          delay: 5000,
          level: 'success',
          persistent: false,
        })
      }
      if (desktop.length) {
        const res = await sapi.patchSettings(Object.fromEntries(desktop.map((k) => [k, r.values[k]])), data.rev)
        toastService.show({
          content: res?.restarting ? t('admin.settings.saved_desktop_app_restarts') : t('admin.settings.settings_saved_successfully'),
          delay: 8000,
          level: 'success',
          persistent: false,
        })
      }
      await load()
    } catch (e: any) {
      const err = sapi.v2Error(e)
      if (err.code === 'rev_conflict') toastService.showError(t('admin.settings.settings_changed_meanwhile_please'))
      else if (Object.keys(err.fields).length) {
        errors = Object.fromEntries(Object.entries(err.fields).map(([k, v]) => [k, (v as string[]).join(' ')]))
        toastService.showError(t('admin.settings.some_values_invalid'))
      } else toastService.showError(err.message, e)
    } finally {
      saving = false
    }
  }

  async function reset(item: SettingItem) {
    try {
      await sapi.resetSetting(item.key)
      await load()
      // Reset saves immediately – otherwise the empty field looks unsaved (feedback 2026-11-17)
      toastService.showInfo(t('admin.settings.reset_saved'))
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }

  // Restart
  let restartPassword = $state('')
  let restarting = $state(false)
  async function doRestart() {
    try {
      await sapi.restart(restartPassword)
      restarting = true
      restartPassword = ''
      const started = Date.now()
      // wait until the server answers again
      await new Promise((r) => setTimeout(r, 3000))
      while (Date.now() - started < 180000) {
        try {
          const res = await fetch('/rs/api/v2/server', { credentials: 'same-origin' })
          if (res.ok) {
            location.reload()
            return
          }
        } catch {}
        await new Promise((r) => setTimeout(r, 2000))
      }
      restarting = false
      toastService.showError(t('admin.settings.paperrust_did_not_come'))
    } catch (e) {
      const err = sapi.v2Error(e)
      toastService.showError(err.fields.password?.join(' ') || err.message, e)
    }
  }

  // Origin & move
  let originItems = $state<OriginItem[] | null>(null)
  let checked = $state<{ kind: string; service: string | null; env_files: string[]; items: OriginItem[]; slim_env: string } | null>(null)
  let importDoc: unknown = null
  let importPass = $state('')
  let importPreview = $state<{ key: string; status: string; current: unknown; new: unknown }[] | null>(null)
  async function loadOrigin() {
    try {
      originItems = (await sapi.origin()).items
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }
  async function adoptKeys(keys: string[]) {
    try {
      const r = await sapi.adopt({ keys })
      toastService.show({ content: t('admin.settings.value_s_moved_database', { PH: String(r.adopted.length) }), delay: 5000, level: 'success', persistent: false })
      await Promise.all([loadOrigin(), load()])
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }
  async function adoptChecked() {
    if (!checked) return
    const values: Record<string, string> = {}
    for (const i of checked.items) if (i.verdict === 'adoptable' && i.value != null) values[i.name] = i.value
    try {
      const r = await sapi.adopt({ values })
      toastService.show({ content: t('admin.settings.value_s_moved_database', { PH: String(r.adopted.length) }), delay: 5000, level: 'success', persistent: false })
      await load()
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }
  async function onCheckFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0]
    if (!f) return
    try {
      checked = await sapi.checkFile(await f.text())
    } catch (err) {
      toastService.showError(sapi.v2Error(err).message, err)
    }
  }
  async function doExport() {
    try {
      const doc = await sapi.exportSettings()
      sapi.saveText('paperrust-settings.json', JSON.stringify(doc, null, 2), 'application/json')
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }
  async function onImportFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0]
    if (!f) return
    try {
      importDoc = JSON.parse(await f.text())
      importPreview = (await sapi.importSettings(importDoc, importPass, true)).items
    } catch (err) {
      importPreview = null
      toastService.showError(sapi.v2Error(err).message, err)
    }
  }
  async function applyImport() {
    try {
      const r = await sapi.importSettings(importDoc, importPass, false)
      toastService.show({ content: t('admin.settings.setting_s_imported', { PH: String(r.applied) }), delay: 5000, level: 'success', persistent: false })
      importPreview = null
      await load()
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }

  // History
  let historyRows = $state<HistoryRow[]>([])
  let historyNext = $state<string | null>(null)
  async function loadHistory(more = false) {
    try {
      const r = await sapi.history(more ? historyNext : null)
      historyRows = more ? [...historyRows, ...r.items] : r.items
      historyNext = r.next_cursor
    } catch (e) {
      toastService.showError(sapi.v2Error(e).message, e)
    }
  }
  function show(v: 'settings' | 'origin' | 'history') {
    view = v
    if (v === 'origin' && !originItems) loadOrigin()
    if (v === 'history' && !historyRows.length) loadHistory()
  }

  const VERDICTS: Record<string, string> = {
    adoptable: t('admin.settings.moved_web_ui'),
    adopted: t('admin.settings.already_in_database_removed'),
    must_stay: t('admin.settings.must_stay_in_environment'),
    no_effect: t('admin.settings.no_effect_in_paperrust'),
    obsolete: t('admin.settings.obsolete'),
    unknown: t('admin.settings.unknown'),
  }

  function lockText(i: SettingItem): string {
    if (i.source === 'env') return t('admin.settings.set_via_environment_cannot', { PH: i.source_detail ?? displayName(i) })
    if (i.source === 'desktop' || i.lock_reason === 'desktop') return t('admin.settings.managed_desktop_app')
    if (i.lock_reason === 'bootstrap') return t('admin.settings.read_startup_set_via')
    if (i.lock_reason === 'environment_only') return t('admin.settings.trust_boundary_host_only')
    if (i.lock_reason === 'no_effect') return t('admin.settings.no_effect_in_paperrust_2')
    return ''
  }
  function plain(v: unknown): string {
    if (v == null) return ''
    if (typeof v === 'string') return v
    return JSON.stringify(v)
  }
</script>

<pngx-page-header>
<PageHeader title={t('common.settings')} info={t('admin.settings.system_settings_apply_all')}></PageHeader>
</pngx-page-header>

<ul class="nav-tabs nav" role="tablist">
  {#each SETTINGS_TABS as [key, label] (key)}
    <li class="nav-item" role="presentation"><a href="{APP_BASE}/settings/{key}" class="nav-link" role="tab" onclick={(e) => goSettings(e, key)}>{label}</a></li>
  {/each}
  <li class="nav-item" role="presentation"><a href="{APP_BASE}/settings/system" class="nav-link active" role="tab" aria-selected="true" onclick={(e) => e.preventDefault()}>{t('common.system')}</a></li>
</ul>

<div class="border-start border-end border-bottom p-3 mb-3 shadow-sm">
  {#if loadError}
    <div class="alert alert-danger">{loadError}</div>
  {:else if !data}
    <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
  {:else}
    {#if data.restart_pending_count > 0}
      <div class="alert alert-warning d-flex flex-wrap align-items-center gap-2">
        <span class="me-auto">{t('admin.settings.change_s_take_effect', { PH: String(data.restart_pending_count) })}</span>
        {#if restarting}
          <span><div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('admin.settings.restarting')}</span>
        {:else if data.restart_supported}
          <input type="password" class="form-control form-control-sm w-auto" placeholder={t('admin.settings.password')} autocomplete="current-password" bind:value={restartPassword} />
          <button type="button" class="btn btn-sm btn-warning" disabled={!restartPassword} onclick={doRestart}>
            <pngx-bs-icon class="me-1" name="arrow-repeat">{@html icon('arrow-repeat')}</pngx-bs-icon>{t('admin.settings.restart_now')}
          </button>
        {:else}
          <span>{t('admin.settings.please_restart_paperrust_service')}</span>
        {/if}
      </div>
    {/if}
    {#if data.mixed_mode}
      <div class="alert alert-info small">{t('admin.settings.mixed_mode_django_values')}</div>
    {/if}

    <div class="btn-group btn-group-sm mb-3" role="group">
      <button type="button" class="btn" class:btn-primary={view === 'settings'} class:btn-outline-primary={view !== 'settings'} onclick={() => show('settings')}>{t('common.settings')}</button>
      <button type="button" class="btn" class:btn-primary={view === 'origin'} class:btn-outline-primary={view !== 'origin'} onclick={() => show('origin')}>{t('admin.settings.origin_migration')}</button>
      <button type="button" class="btn" class:btn-primary={view === 'history'} class:btn-outline-primary={view !== 'history'} onclick={() => show('history')}>{t('common.history')}</button>
    </div>

    {#if view === 'settings'}
      <div class="row">
        <div class="col-md-3 mb-3">
          <div class="input-group input-group-sm mb-2">
            <span class="input-group-text"><pngx-bs-icon name="search">{@html icon('search')}</pngx-bs-icon></span>
            <input type="search" class="form-control" placeholder={t('admin.settings.search_settings')} bind:value={filter.q} />
          </div>
          <div class="form-check form-switch small"><input class="form-check-input" type="checkbox" id="sys-changed" bind:checked={filter.changed} /><label class="form-check-label" for="sys-changed">{t('admin.settings.only_changed')}</label></div>
          <div class="form-check form-switch small"><input class="form-check-input" type="checkbox" id="sys-restart" bind:checked={filter.restart} /><label class="form-check-label" for="sys-restart">{t('admin.settings.only_restart_required')}</label></div>
          <div class="form-check form-switch small mb-2"><input class="form-check-input" type="checkbox" id="sys-ro" bind:checked={filter.readOnly} /><label class="form-check-label" for="sys-ro">{t('admin.settings.show_startup_environment_only')}</label></div>
          {#if !searching}
            <div class="list-group list-group-flush">
              {#each GROUPS.filter((g) => counts[g.id]) as g (g.id)}
                <a href="{APP_BASE}/settings/system/{g.id}" class="list-group-item list-group-item-action d-flex justify-content-between align-items-center py-1" class:fw-bold={g.id === group} class:text-primary={g.id === group} aria-current={g.id === group ? 'page' : undefined} onclick={(e) => goGroup(e, g.id)}>
                  {t(g.label)}<span class="badge rounded-pill text-bg-light">{counts[g.id]}</span>
                </a>
              {/each}
            </div>
          {/if}
        </div>
        <div class="col-md-9">
          {#if !shown.length}
            <p class="text-muted">{t('admin.settings.no_settings_match')}</p>
          {/if}
          {#each shown as i (i.key)}
            {@const ro = !editable(i)}
            <div class="border-bottom pb-2 mb-3">
              <div class="d-flex flex-wrap align-items-center gap-1 mb-1">
                <label class="fw-semibold me-1" for="sys-{i.key}">{helpText(i)}</label>
                <a href={docsUrl(`configuration#${displayName(i)}`)} target="_blank" rel="noopener" title={t('common.documentation')} class="text-muted"><pngx-bs-icon name="question-circle">{@html icon('question-circle')}</pngx-bs-icon></a>
                {#if i.source === 'env'}<span class="badge text-bg-secondary" title={i.source_detail ?? ''}>{t('admin.settings.environment')}</span>{/if}
                {#if i.source === 'desktop'}<span class="badge text-bg-secondary">{t('admin.settings.desktop_app')}</span>{/if}
                {#if i.source === 'appconfig' && i.shadowed}<span class="badge text-bg-info">{t('admin.settings.overrides_environment')}</span>{/if}
                {#if i.source === 'db' && i.shadowed?.source === 'file'}<span class="badge text-bg-info">{t('admin.settings.overrides_paperless_conf')}</span>{/if}
                {#if i.source === 'file'}<span class="badge text-bg-light" title={i.source_detail ?? ''}>{fileName(i.source_detail)}</span>{/if}
                {#if i.restart_pending}<span class="badge text-bg-warning">{t('admin.settings.restart_required')}</span>{/if}
                {#if i.reindex}<span class="badge text-bg-light">{t('admin.settings.rebuild_index')}</span>{/if}
                {#if i.secret}<span class="badge text-bg-dark">{t('admin.settings.secret')}</span>{/if}
                {#if i.class === 'X'}<span class="badge text-bg-light text-muted">{t('admin.settings.mixed_mode_only')}</span>{/if}
                {#if data.mixed_mode && i.django_consumer && (i.source === 'db' || i.source === 'appconfig')}<span class="badge text-bg-light">{t('admin.settings.not_applied_django_paths')}</span>{/if}
                {#if i.unreadable}<span class="badge text-bg-danger">{t('admin.settings.secret_unreadable_please_enter')}</span>{/if}
              </div>
              <div class="small text-muted mb-1"><code class="user-select-all">{displayName(i)}</code>{#each formerNames(i) as a (a)} <span>({t('admin.settings.former_name')}: <code>{a}</code>)</span>{/each}
                {#if i.effect === 'server' && !i.restart_pending && !ro} · {t('admin.settings.takes_effect_after_restart')}{/if}
                {#if i.updated_by} · {t('admin.settings.changed', { PH: i.updated_by })}{/if}
              </div>
              {#if ro && !(i.class === 'A' || i.class === 'B') && !i.key.startsWith('desktop.')}
                <div class="small"><span class="text-muted">{lockText(i)}</span>{#if !i.secret && i.value != null} <code>{plain(i.value)}</code>{/if}{#if i.secret} <span class="text-muted">({i.is_set ? t('admin.settings.set') : t('admin.settings.not_set')})</span>{/if}</div>
              {:else}
                <div class="d-flex flex-wrap gap-2 align-items-start">
                  <div class="flex-grow-1">
                    {#if i.type.kind === 'bool' || i.type.kind === 'on_off'}
                      <div class="form-check form-switch"><input class="form-check-input" type="checkbox" id="sys-{i.key}" disabled={ro} checked={form[i.key] === true} onchange={(e) => (form[i.key] = (e.target as HTMLInputElement).checked)} /></div>
                    {:else if i.type.kind === 'choice'}
                      <select class="form-select form-select-sm" id="sys-{i.key}" disabled={ro} bind:value={form[i.key]}>
                        {#if !i.type.choices.includes(String(form[i.key]))}<option value={form[i.key]}>{form[i.key] || '–'}</option>{/if}
                        {#each i.type.choices as c (c)}<option value={c}>{c}</option>{/each}
                      </select>
                    {:else if i.type.kind === 'json'}
                      <textarea class="form-control form-control-sm font-monospace" rows="3" id="sys-{i.key}" disabled={ro} bind:value={form[i.key]}></textarea>
                    {:else if i.secret}
                      <input type="password" class="form-control form-control-sm" id="sys-{i.key}" autocomplete="new-password" disabled={ro} placeholder={i.is_set ? t('admin.settings.set_leave_empty_keep') : t('admin.settings.not_set')} bind:value={form[i.key]} />
                    {:else if i.type.kind === 'int' || i.type.kind === 'float'}
                      <input type="number" class="form-control form-control-sm" id="sys-{i.key}" disabled={ro} min={i.type.min ?? undefined} max={i.type.kind === 'int' ? (i.type.max ?? undefined) : undefined} step={i.type.kind === 'float' ? 'any' : '1'} bind:value={form[i.key]} />
                    {:else}
                      <input type="text" class="form-control form-control-sm" id="sys-{i.key}" disabled={ro} placeholder={i.default != null ? plain(i.default) : ''} bind:value={form[i.key]} list={i.key.endsWith('LLM_OUTPUT_LANGUAGE') ? 'sys-llm-languages' : undefined} />
                      {#if i.key.endsWith('LLM_OUTPUT_LANGUAGE')}<datalist id="sys-llm-languages">{#each LLM_LANGUAGE_CHOICES as c (c.id)}<option value={c.id}>{c.name}</option>{/each}</datalist>{/if}
                      {#if i.type.kind === 'cron'}
                        {@const c = cronText(String(form[i.key] || i.default || ''))}
                        {#if c}<div class="form-text">{t(c.text, c.params)}</div>{/if}
                      {:else if i.type.kind === 'list'}
                        <div class="form-text">{t('admin.settings.comma_separated')}</div>
                      {/if}
                    {/if}
                    {#if errors[i.key]}<div class="text-danger small">{t(errors[i.key])}</div>{/if}
                    {#if ro}<div class="form-text">{lockText(i)}</div>{/if}
                    {#if i.shadowed && !i.secret}<div class="form-text">{i.source === 'appconfig' || i.source === 'db' ? t('admin.settings.environment_file_value_label') : t('admin.settings.database_value_label')} <code>{plain(i.shadowed.value)}</code></div>{/if}
                    {#if i.secret && !ro && (i.source === 'db' || i.source === 'appconfig')}<div class="form-text">{i.appconfig_field ? t('admin.settings.stored_unencrypted_in_database') : t('admin.settings.stored_encrypted_in_database')}</div>{/if}
                  </div>
                  {#if !ro && (i.source === 'db' || i.source === 'appconfig')}
                    <button type="button" class="btn btn-sm btn-outline-secondary" title={t('admin.settings.reset_default')} onclick={() => reset(i)}><pngx-bs-icon name="arrow-counterclockwise">{@html icon('arrow-counterclockwise')}</pngx-bs-icon></button>
                  {/if}
                </div>
              {/if}
            </div>
          {/each}
          <button type="button" class="btn btn-primary" disabled={saving || !dirtyKeys.length} onclick={save}>
            {#if saving}<div class="spinner-border spinner-border-sm me-2" role="status"></div>{/if}{t('common.save')}
          </button>
          {#if dirtyKeys.length}
            <button type="button" class="btn btn-outline-secondary ms-2" onclick={() => ((form = { ...initial }), (errors = {}))}>{t('common.discard')}</button>
          {/if}
        </div>
      </div>
    {:else if view === 'origin'}
      <p class="small text-muted">{t('admin.settings.values_environment_paperless_conf')}</p>
      <div class="d-flex flex-wrap gap-2 mb-3">
        <button type="button" class="btn btn-sm btn-outline-primary" disabled={!originItems?.some((i) => i.verdict === 'adoptable')} onclick={() => adoptKeys(originItems!.filter((i) => i.verdict === 'adoptable').map((i) => i.name))}>{t('admin.settings.move_all_database')}</button>
        <a class="btn btn-sm btn-outline-primary" href="{sapi.BASE}/slim-env"><pngx-bs-icon class="me-1" name="download">{@html icon('download')}</pngx-bs-icon>{t('admin.settings.download_slim_configuration')}</a>
        <button type="button" class="btn btn-sm btn-outline-primary" onclick={doExport}><pngx-bs-icon class="me-1" name="download">{@html icon('download')}</pngx-bs-icon>{t('admin.settings.export_settings')}</button>
      </div>
      {#if originItems}
        <div class="table-responsive">
          <table class="table table-sm align-middle small">
            <thead><tr><th>{t('admin.settings.variable')}</th><th>{t('admin.settings.value')}</th><th>{t('admin.settings.source')}</th><th>{t('admin.settings.assessment')}</th><th></th></tr></thead>
            <tbody>
              {#each originItems as o (o.name)}
                <tr>
                  <td><code>{o.name}</code></td>
                  <td class="text-break">{o.secret ? '***' : (o.value ?? '')}</td>
                  <td>{o.source}</td>
                  <td>{VERDICTS[o.verdict]}{#if o.reason && o.verdict !== 'adopted'}<div class="text-muted">{o.reason}</div>{/if}</td>
                  <td>{#if o.verdict === 'adoptable'}<button type="button" class="btn btn-sm btn-outline-primary text-nowrap" onclick={() => adoptKeys([o.name])}>{t('admin.settings.move_database')}</button>{/if}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      <h6 class="mt-4">{t('admin.settings.check_configuration_file')}</h6>
      <p class="small text-muted">{t('admin.settings.paperless_conf_docker_compose')}</p>
      <input type="file" class="form-control form-control-sm mb-2" accept=".conf,.env,.yml,.yaml,text/plain" onchange={onCheckFile} />
      {#if checked}
        {#if checked.service}<p class="small">{t('admin.settings.service', { PH: checked.service })}{#if checked.env_files.length} · env_file: <code>{checked.env_files.join(', ')}</code> – {t('admin.settings.please_check_file_too')}{/if}</p>{/if}
        <div class="table-responsive">
          <table class="table table-sm small">
            <tbody>
              {#each checked.items as o (o.name)}
                <tr><td><code>{o.name}</code></td><td>{VERDICTS[o.verdict]}{#if o.reason}<div class="text-muted">{o.reason}</div>{/if}</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
        <button type="button" class="btn btn-sm btn-outline-primary me-2" disabled={!checked.items.some((i) => i.verdict === 'adoptable')} onclick={adoptChecked}>{t('admin.settings.move_all_database')}</button>
        <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => sapi.saveText('docker-compose.env', checked!.slim_env)}>{t('admin.settings.download_slim_configuration')}</button>
      {/if}
      <h6 class="mt-4">{t('admin.settings.import_settings')}</h6>
      <div class="d-flex flex-wrap gap-2 mb-2">
        <input type="password" class="form-control form-control-sm w-auto" placeholder={t('admin.settings.passphrase_encrypted_files')} bind:value={importPass} />
        <input type="file" class="form-control form-control-sm w-auto" accept=".json,application/json" onchange={onImportFile} />
      </div>
      {#if importPreview}
        <table class="table table-sm small">
          <tbody>
            {#each importPreview as r (r.key)}
              <tr><td><code>{displayKey(r.key)}</code></td><td>{r.status}</td><td>{plain(r.current)} → {plain(r.new)}</td></tr>
            {/each}
          </tbody>
        </table>
        <button type="button" class="btn btn-sm btn-primary" disabled={!importPreview.some((r) => r.status === 'change')} onclick={applyImport}>{t('admin.settings.apply_import')}</button>
      {/if}
    {:else}
      <div class="table-responsive">
        <table class="table table-sm small">
          <thead><tr><th>{t('common.date')}</th><th>{t('admin.settings.setting')}</th><th>{t('common.action')}</th><th>{t('admin.settings.before')}</th><th>{t('admin.settings.after')}</th><th>{t('common.user')}</th></tr></thead>
          <tbody>
            {#each historyRows as h (h.id)}
              <tr><td class="text-nowrap">{new Date(h.at).toLocaleString()}</td><td><code>{displayKey(h.key)}</code></td><td>{h.action}</td><td class="text-break">{h.old_value ?? ''}</td><td class="text-break">{h.new_value ?? ''}</td><td>{h.user_name ?? ''}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if historyNext}<button type="button" class="btn btn-sm btn-outline-secondary" onclick={() => loadHistory(true)}>{t('admin.settings.load_more')}</button>{/if}
    {/if}
  {/if}
</div>
