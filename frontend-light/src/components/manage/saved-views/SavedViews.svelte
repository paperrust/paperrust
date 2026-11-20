<!-- Port of manage/saved-views/saved-views.component.html (Route savedviews). Host <pngx-saved-views> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/saved-views.css'
  import { onDestroy } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { modals } from '../../../lib/modal.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { data } from '../../../stores/data.svelte'
  import { DEFAULT_DISPLAY_FIELDS } from '../../../stores/listView.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { savedViewsOrganizing } from '../../../stores/savedViewsOrganizing.svelte'
  import { api } from '../../../api/client'
  import { patchOf } from '../../../api/admin'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import InputText from '../../common/input/text/InputText.svelte'
  import InputSelect from '../../common/input/select/InputSelect.svelte'
  import InputNumber from '../../common/input/number/InputNumber.svelte'
  import DragDropSelect from '../../common/input/drag-drop-select/DragDropSelect.svelte'
  import ConfirmButton from '../../common/confirm-button/ConfirmButton.svelte'
  import Pagination from '../../common/pagination/Pagination.svelte'
  import PermissionsDialog from '../../common/permissions-dialog/PermissionsDialog.svelte'
  import { SAVED_VIEW_ICONS } from './saved-view-icons'
  import { SavedViewsState, type FullSavedView } from './saved-views.svelte'

  const s = new SavedViewsState()
  s.reload()

  const savedViewIcons = SAVED_VIEW_ICONS.map((i) => ({ ...i, name: t(i.name) }))
  const canSaveSettings = session.can('change', 'uisettings') && session.can('add', 'uisettings')
  if (canSaveSettings) savedViewsOrganizing.active = true
  onDestroy(() => (savedViewsOrganizing.active = false))

  /** SettingsService.allDisplayFields */
  const displayFields = $derived.by(() => {
    const notesEnabled = session.get(SETTINGS_KEYS.NOTES_ENABLED)
    const perm: Record<string, string> = { tag: 'tag', correspondent: 'correspondent', documenttype: 'documenttype', storagepath: 'storagepath', owner: 'user', note: 'note' }
    const base = DEFAULT_DISPLAY_FIELDS.filter((f) => {
      if (f.id === 'note' && !notesEnabled) return false
      if (['title', 'created', 'added', 'asn', 'pagecount', 'shared'].includes(f.id)) return true
      return session.can('view', perm[f.id])
    }).map((f) => ({ id: f.id, name: t(f.name) }))
    const cfs = session.can('view', 'customfield') ? data.customFields.map((c) => ({ id: `custom_field_${c.id}`, name: c.name })) : []
    return [...base, ...cfs]
  })

  const viewOf = (id: number) => s.views?.find((v) => v.id === id) as FullSavedView
  const canEdit = (id: number) => session.hasObjectPermissions('change', viewOf(id))
  const canDelete = (id: number) => session.ownsObject(viewOf(id))

  /** SavedViewService.clearCache: also refresh sidebar/home page */
  async function refreshShared() {
    try {
      data.savedViews = (await api.listAll<any>('saved_views')).results
      data.refreshViewCounts()
    } catch {}
  }

  async function save(e: Event) {
    e.preventDefault()
    try {
      if (!(await s.save(canEdit, canSaveSettings))) return
      toastService.showSuccess(t('manage.views.views_saved_successfully'))
      await refreshShared()
      s.reload()
    } catch (error) {
      toastService.showError(t('manage.views.error_while_saving_views'), error)
    }
  }

  async function deleteSavedView(view: FullSavedView) {
    await s.remove(view)
    toastService.showSuccess(t('manage.views.saved_view_deleted', { PH: view.name }))
    await refreshShared()
    s.reload()
  }

  function editPermissions(view: FullSavedView) {
    const ref = modals.open(
      PermissionsDialog,
      {
        object: view,
        note: t('manage.views.note_sharing_saved_views'),
        onconfirm: async ({ permissions }: any, setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await patchOf('saved_views', { id: view.id, owner: permissions.owner, set_permissions: permissions.set_permissions })
            toastService.showSuccess(t('common.permissions_updated'))
            ref.close()
            s.reload()
          } catch (error) {
            toastService.showError(t('common.error_updating_permissions'), error)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
</script>

<pngx-page-header>
<PageHeader
  title={t('common.saved_views')}
  info={t('manage.views.customize_views_documents')}
  >
</PageHeader>
</pngx-page-header>
<form onsubmit={save}>
  <ul class="list-group mb-3">
    {#each s.pagedForms as view (view.id)}
      {@const editable = canEdit(view.id)}
      <li class="list-group-item py-3">
      <div>
        <div class="row">
          <div class="col-md">
            <pngx-input-text><InputText title="Name" bind:value={view.name} disabled={!editable} /></pngx-input-text>
          </div>
          <div class="col-md">
            <pngx-input-select><InputSelect
            title={t('manage.views.icon')}
            bind:value={view.icon}
            items={savedViewIcons}
            iconField="icon"
            disabled={!editable} /></pngx-input-select>
          </div>
          {#if canSaveSettings}
            <div class="col-md">
              <div class="form-check form-switch mt-3">
                <input type="checkbox" class="form-check-input" id="show_on_dashboard_{view.id}" bind:checked={view.show_on_dashboard}>
                <label class="form-check-label" for="show_on_dashboard_{view.id}">{t('manage.views.show_dashboard')}</label>
              </div>
              <div class="form-check form-switch">
                <input type="checkbox" class="form-check-input" id="show_in_sidebar_{view.id}" bind:checked={view.show_in_sidebar}>
                <label class="form-check-label" for="show_in_sidebar_{view.id}">{t('manage.views.show_in_sidebar')}</label>
              </div>
            </div>
          {/if}
          <div class="col-auto">
            {#if canDelete(view.id)}
              <label class="form-label" for="name_{view.id}">{t('common.actions')}</label>
              {#if session.can('change', 'savedview')}
                <button
                  class="btn btn-sm btn-outline-secondary form-control mb-2"
                  type="button"
                  onclick={() => editPermissions(viewOf(view.id))}
                  ><pngx-bs-icon class="me-1" name="person-fill-lock">{@html icon('person-fill-lock')}</pngx-bs-icon>{t('common.permissions')}</button>
              {/if}
              {#if session.can('delete', 'savedview')}
                <pngx-confirm-button><ConfirmButton
                  label={t('common.delete')}
                  onconfirm={() => deleteSavedView(viewOf(view.id))}
                  buttonClasses="btn-sm btn-outline-danger form-control"
                  iconName="trash" /></pngx-confirm-button>
              {/if}
            {/if}
          </div>
        </div>
        <div class="row">
          <div class="col">
            <pngx-input-number><InputNumber title={t('manage.views.documents_page_size')} showAdd={false} bind:value={view.page_size} disabled={!editable} /></pngx-input-number>
          </div>
          <div class="col">
            <label class="form-label" for="display_mode_{view.id}">{t('manage.views.display')}</label>
            <select class="form-select form-control" bind:value={view.display_mode} disabled={!editable}>
              <option value="table">{t('manage.views.table')}</option>
              <option value="smallCards">{t('manage.views.small_cards')}</option>
              <option value="largeCards">{t('manage.views.large_cards')}</option>
            </select>
          </div>
            {#if displayFields}
              <pngx-input-drag-drop-select><DragDropSelect title={t('common.show')} emptyText={t('common.default')} items={displayFields} bind:value={view.display_fields} disabled={!editable} /></pngx-input-drag-drop-select>
            {/if}
            <span class="small text-muted fst-italic">{t('manage.views.note_ordering_not_preserved')}</span>
          </div>
      </div>
      </li>
    {/each}

    {#if s.views && s.views.length === 0}
      <li class="list-group-item">
        <div>{t('manage.views.no_saved_views_defined')}</div>
      </li>
    {/if}

    {#if s.loading}
      <li class="list-group-item">
        <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
        <div class="visually-hidden">{t('common.loading')}</div>
      </li>
    {/if}
  </ul>

  <div class="d-flex align-items-center mb-3">
    <button type="button" onclick={() => s.reset()} class="btn btn-outline-secondary mb-2" disabled={!s.dirty}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary ms-2 mb-2" disabled={!s.dirty}>{t('common.save')}</button>
    {#if (s.views?.length ?? 0) > s.pageSize}
      <ngb-pagination class="ms-auto" size="sm" aria-label="Pagination" role="navigation"><Pagination pageSize={s.pageSize} collectionSize={s.views?.length ?? 0} page={s.page} maxSize={5} onpage={(p) => (s.page = p)} /></ngb-pagination>
    {/if}
  </div>
</form>
