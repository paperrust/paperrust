<!-- Port of common/permissions-dialog/permissions-dialog.component.html (bulk permissions). Reports back via
     onconfirm({ permissions, merge }, setButtonsEnabled). -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { data } from '../../../stores/data.svelte'
  import type { ObjectPermissionsPayload } from '../../../api/objects'
  import PermissionsForm from '../input/permissions/permissions-form/PermissionsForm.svelte'
  import InputSwitch from '../input/switch/InputSwitch.svelte'

  let {
    modal,
    object = undefined,
    note = null,
    onconfirm,
  }: {
    modal: ModalRef<any>
    object?: { name?: string; owner?: number | null; permissions?: any }
    /** dialog.note (e.g. saved views) */
    note?: string | null
    onconfirm?: (r: { permissions: ObjectPermissionsPayload; merge: boolean }, setButtonsEnabled: (b: boolean) => void) => void
  } = $props()

  const users = $derived([...data.users.values()])
  // $localize`Edit permissions for ` ends with a space (the i18n key is trimmed)
  const title = object ? t('ui.edit_permissions').replace(/\s*$/, ' ') + object['name'] : t('ui.set_permissions')
  let buttonsEnabled = $state(true)
  let merge = $state(true)
  let permissionsForm = $state<any>(object ? { owner: object.owner, set_permissions: object.permissions } : undefined)
  const hint = $derived(
    object ? null : merge ? t('ui.existing_owner_user_group') : t('ui.any_all_existing_owner'),
  )
  const permissions = (): ObjectPermissionsPayload => ({
    owner: permissionsForm?.owner ?? null,
    set_permissions: permissionsForm?.set_permissions ?? { view: { users: [], groups: [] }, change: { users: [], groups: [] } },
  })
  function confirm() {
    onconfirm?.({ permissions: permissions(), merge }, (b) => (buttonsEnabled = b))
  }
</script>

<pngx-permissions-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}>
  </button>
</div>
<div class="modal-body">

  <form>
    <div class="form-group">
      <pngx-permissions-form><PermissionsForm {users} bind:value={permissionsForm} /></pngx-permissions-form>
    </div>
    <div class="form-group mt-4">
      <div class="offset-lg-3 row">
        <pngx-input-switch><InputSwitch title={t('ui.merge_existing_permissions')} horizontal={true} {hint} bind:value={merge} /></pngx-input-switch>
      </div>
    </div>
  </form>

  {#if note}
    <div class="small text-muted fst-italic mt-2">
      {note}
    </div>
  {/if}

</div>
<div class="modal-footer">
  {#if !buttonsEnabled}
    <div class="spinner-border spinner-border-sm me-2" role="status"></div>
    <span class="visually-hidden">{t('common.loading')}</span>
  {/if}
  <button type="button" class="btn btn-outline-primary" onclick={() => modal.close()} disabled={!buttonsEnabled}>{t('common.cancel')}</button>
  <button type="button" class="btn btn-primary" onclick={confirm} disabled={!buttonsEnabled}>{t('common.confirm')}</button>
</div></pngx-permissions-dialog>
