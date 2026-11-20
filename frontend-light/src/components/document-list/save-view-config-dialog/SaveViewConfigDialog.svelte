<!-- Port of document-list/save-view-config-dialog (save-view-config-dialog.component.html/.ts).
     Angular: saveClicked output → here onsave(formValue); if onsave returns an error (HTTP error body), it is
     shown like modal.componentInstance.error.set(error) and the buttons become active again. -->
<script lang="ts" module>
  export interface SaveViewConfigValue {
    name: string
    icon: string
    showInSideBar: boolean
    showOnDashboard: boolean
    permissions_form?: { owner: number | null; set_permissions: any }
  }
</script>

<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { data } from '../../../stores/data.svelte'
  import InputText from '../../common/input/text/InputText.svelte'
  import InputSelect from '../../common/input/select/InputSelect.svelte'
  import InputCheck from '../../common/input/check/InputCheck.svelte'
  import PermissionsForm from '../../common/input/permissions/permissions-form/PermissionsForm.svelte'
  import { DEFAULT_SAVED_VIEW_ICON, SAVED_VIEW_ICONS } from '../../manage/saved-views/saved-view-icons'

  let {
    modal,
    defaultName = '',
    onsave,
  }: {
    modal: ModalRef<any>
    defaultName?: string
    /** Return: error object (e.g. { name: [...], filter_rules: [...] }) or nothing on success */
    onsave: (value: SaveViewConfigValue) => Promise<any>
  } = $props()

  const savedViewIcons = SAVED_VIEW_ICONS.map((i) => ({ ...i, name: t(i.name) }))
  const users = $derived([...data.users.values()])

  // svelte-ignore state_referenced_locally
  let name = $state(defaultName)
  let icon = $state(DEFAULT_SAVED_VIEW_ICON)
  let showInSideBar = $state(false)
  let showOnDashboard = $state(false)
  let permissionsForm = $state<SaveViewConfigValue['permissions_form'] | undefined>(undefined)
  let error = $state<any>(undefined)
  let buttonsEnabled = $state(true)
  // Enable the close button only after the first render (focus stays in the input field)
  let closeEnabled = $state(false)
  $effect(() => {
    const id = setTimeout(() => (closeEnabled = true))
    return () => clearTimeout(id)
  })

  async function save(e: Event) {
    e.preventDefault()
    const value: SaveViewConfigValue = { name: name ?? '', icon, showInSideBar: !!showInSideBar, showOnDashboard: !!showOnDashboard }
    if (permissionsForm) value.permissions_form = permissionsForm
    buttonsEnabled = false
    const err = await onsave(value)
    if (err) {
      error = err
      buttonsEnabled = true
    }
  }
  function cancel() {
    modal.close()
  }
</script>

<pngx-save-view-config-dialog><form onsubmit={save} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('dialog.save_view.title')}</h4>
    <button type="button" disabled={!closeEnabled} class="btn-close" aria-label="Close" onclick={cancel}>
    </button>
  </div>
  <div class="modal-body">
    <pngx-input-text><InputText title={t('common.name')} bind:value={name} error={error?.name} autocomplete="off" /></pngx-input-text>
    <pngx-input-select><InputSelect
      title={t('manage.views.icon')}
      bind:value={icon}
      items={savedViewIcons}
      iconField="icon"
      error={error?.icon} /></pngx-input-select>
    <pngx-input-check><InputCheck title={t('manage.views.show_in_sidebar')} bind:value={showInSideBar} /></pngx-input-check>
    <pngx-input-check><InputCheck title={t('manage.views.show_dashboard')} bind:value={showOnDashboard} /></pngx-input-check>
    <pngx-permissions-form accordion="true"><PermissionsForm {users} accordion={true} bind:value={permissionsForm} /></pngx-permissions-form>
    {#if error?.filter_rules}
      <div class="alert alert-danger" role="alert">
        <h6 class="alert-heading">{t('dialog.save_view.filter_rules_error')}</h6>
        {t('dialog.save_view.error_message')}:<br/>
        {error.filter_rules}
      </div>
    {/if}
  </div>
  <div class="modal-footer">
    <button type="button" class="btn btn-outline-secondary" onclick={cancel} disabled={!buttonsEnabled}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={!buttonsEnabled}>{t('common.save')}</button>
  </div>
</form></pngx-save-view-config-dialog>
