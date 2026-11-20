<!-- Port of common/edit-dialog/group-edit-dialog/group-edit-dialog.component.html (dialog via modals.open, size lg). -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import PermissionsSelect from '../../permissions-select/PermissionsSelect.svelte'

  let props: EditDialogProps<any> = $props()
  const dlg = new EditDialogState<any>('groups' as any, props, { name: '', permissions: [] })
  const title = $derived(dlg.mode === 'create' ? t('dialog.group.create_new_user_group') : t('dialog.group.edit_user_group'))
</script>

<pngx-group-edit-dialog><form onsubmit={(e) => { e.preventDefault(); dlg.save() }} autocomplete="off">
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
        <div class="col">
          <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
          <pngx-permissions-select><PermissionsSelect title={t('common.permissions')} bind:value={dlg.form.permissions} error={dlg.error?.permissions} /></pngx-permissions-select>
        </div>
      </div>
    </div>
    <div class="modal-footer">
      <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
      <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
    </div>
  </form></pngx-group-edit-dialog>
