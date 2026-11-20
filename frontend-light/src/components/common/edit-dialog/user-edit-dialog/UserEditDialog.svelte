<!-- Port of common/edit-dialog/user-edit-dialog/user-edit-dialog.component.html (dialog via modals.open, size xl).
     Disabled fields (permissions for superuser, superuser switch for non-superusers) are missing from the sent value, as in Angular. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { api } from '../../../../api/client'
  import { listAllOf } from '../../../../api/admin'
  import { session } from '../../../../stores/settings.svelte'
  import { toastService } from '../../../../stores/toasts.svelte'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputPassword from '../../input/password/InputPassword.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import ConfirmButton from '../../confirm-button/ConfirmButton.svelte'
  import PermissionsSelect from '../../permissions-select/PermissionsSelect.svelte'
  import { passwordIsSet, userFormValues } from './user-edit'

  let props: EditDialogProps<any> & { onpasswordset?: (set: boolean) => void } = $props()
  const dlg = new EditDialogState<any>('users' as any, props, {
    username: '',
    email: '',
    password: null,
    first_name: '',
    last_name: '',
    is_active: true,
    is_staff: true,
    is_superuser: false,
    groups: [],
    user_permissions: [],
  })
  const title = $derived(dlg.mode === 'create' ? t('dialog.user.create_new_user_account') : t('dialog.user.edit_user_account'))
  // When editing, focus lands on the close button in Angular (NgbModalWindow focuses only after the
  // setTimeout that sets closeEnabled), when creating on the username – measured, replicated here.
  if (dlg.mode === 'edit') dlg.closeEnabled = true
  const currentUserIsSuperUser = !!(session.data?.user as any)?.is_superuser
  let groups = $state<any[] | undefined>(undefined)
  listAllOf<any>('groups')
    .then((r) => (groups = r.results))
    .catch((e) => {
      toastService.showError(t('common.error_retrieving_groups'), e)
      groups = []
    })
  const inheritedPermissions = $derived((dlg.form.groups ?? []).flatMap((id: number) => groups?.find((g) => g.id == id)?.permissions ?? []))
  let totpLoading = $state(false)
  let mfaEnabled = $state(!!dlg.object?.is_mfa_enabled)

  function save() {
    props.onpasswordset?.(passwordIsSet(dlg.form.password))
    dlg.save(() => userFormValues($state.snapshot(dlg.form), currentUserIsSuperUser))
  }

  async function deactivateTotp() {
    totpLoading = true
    try {
      const result = await api.request<boolean>('POST', `/api/users/${dlg.object.id}/deactivate_totp/`, null)
      totpLoading = false
      if (result) {
        toastService.showSuccess(t('dialog.user.totp_deactivated'))
        mfaEnabled = false
      } else toastService.showError(t('dialog.user.totp_deactivation_failed'))
    } catch (e) {
      totpLoading = false
      toastService.showError(t('dialog.user.totp_deactivation_failed'), e)
    }
  }
</script>

<pngx-user-edit-dialog><form onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off">
    <div class="modal-header">
      <h4 class="modal-title" id="modal-basic-title">{title}</h4>
      {#if dlg.object?.id}
        <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {dlg.object.id}</span>
      {/if}
      <button type="button" disabled={!dlg.closeEnabled} class="btn-close" aria-label="Close" onclick={() => dlg.cancel()}>
      </button>
    </div>
    <div class="modal-body">
      <div class="row">
        <div class="col-md-4">
          <pngx-input-text><InputText title={t('common.username')} bind:value={dlg.form.username} error={dlg.error?.username} /></pngx-input-text>
          <pngx-input-text><InputText title={t('common.email')} bind:value={dlg.form.email} error={dlg.error?.email} /></pngx-input-text>
          <pngx-input-password><InputPassword title={t('common.password')} bind:value={dlg.form.password} error={dlg.error?.password} /></pngx-input-password>
          <pngx-input-text><InputText title={t('dialog.user.first_name')} bind:value={dlg.form.first_name} error={dlg.error?.first_name} /></pngx-input-text>
          <pngx-input-text><InputText title={t('dialog.user.last_name')} bind:value={dlg.form.last_name} error={dlg.error?.first_name} /></pngx-input-text>

          <div class="mb-2 d-flex flex-column">
            <div class="form-check form-switch form-check-inline">
              <input type="checkbox" class="form-check-input" id="is_active" bind:checked={dlg.form.is_active}>
              <label class="form-check-label" for="is_active">{t('dialog.user.active')}</label>
            </div>
            <div class="form-check form-switch form-check-inline">
              <input type="checkbox" class="form-check-input" id="is_staff" bind:checked={dlg.form.is_staff}>
              <label class="form-check-label" for="is_staff">{t('dialog.user.admin')}<small class="form-text text-muted ms-1">{t('dialog.user.access_system_status_logs')}</small></label>
            </div>
            <div class="form-check form-switch form-check-inline">
              <input type="checkbox" class="form-check-input" id="is_superuser" bind:checked={dlg.form.is_superuser} disabled={!currentUserIsSuperUser}>
              <label class="form-check-label" for="is_superuser">{t('dialog.user.superuser')}<small class="form-text text-muted ms-1">{t('dialog.user.grants_all_permissions_view')}</small></label>
            </div>
          </div>

          <pngx-input-select><InputSelect title={t('common.groups')} items={groups} multiple={true} bind:value={dlg.form.groups} /></pngx-input-select>

          {#if mfaEnabled && currentUserIsSuperUser}
            <label class="form-label">{t('dialog.user.two_factor_authentication')}</label>
            <pngx-confirm-button title={t('dialog.user.disable_two_factor_authentication')}><ConfirmButton
              label={t('dialog.user.disable_two_factor_authentication')}
              buttonClasses="btn-outline-danger btn-sm"
              iconName="trash"
              disabled={totpLoading}
              onconfirm={deactivateTotp} /></pngx-confirm-button>
          {/if}
        </div>
        <div class="col">
          <pngx-permissions-select><PermissionsSelect title={t('common.permissions')} bind:value={dlg.form.user_permissions} error={dlg.error?.user_permissions} {inheritedPermissions} disabled={!!dlg.form.is_superuser} /></pngx-permissions-select>
        </div>
      </div>
    </div>
    <div class="modal-footer">
      <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
      <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
    </div>
  </form></pngx-user-edit-dialog>
