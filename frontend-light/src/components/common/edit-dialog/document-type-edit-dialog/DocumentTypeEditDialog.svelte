<!-- Port of common/edit-dialog/document-type-edit-dialog/document-type-edit-dialog.component.html. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { data } from '../../../../stores/data.svelte'
  import { DEFAULT_MATCHING_ALGORITHM, matchingAlgorithms, patternRequired, type MatchingModel } from '../../../../lib/matching'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import InputCheck from '../../input/check/InputCheck.svelte'
  import PermissionsForm from '../../input/permissions/permissions-form/PermissionsForm.svelte'

  let props: EditDialogProps<MatchingModel> = $props()
  const dlg = new EditDialogState<MatchingModel>('document_types', props, {
    name: '',
    matching_algorithm: DEFAULT_MATCHING_ALGORITHM,
    match: '',
    is_insensitive: true,
    permissions_form: null,
  })
  const users = $derived([...data.users.values()])
  const title = $derived(dlg.mode === 'create' ? t('dialog.document_type.create_new_document_type') : t('dialog.document_type.edit_document_type'))
</script>

<pngx-document-type-edit-dialog><form onsubmit={(e) => { e.preventDefault(); dlg.save() }} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    {#if dlg.object?.id}
      <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {dlg.object.id}</span>
    {/if}
    <button type="button" disabled={!dlg.closeEnabled} class="btn-close" aria-label="Close" onclick={() => dlg.cancel()}>
    </button>
  </div>
  <div class="modal-body">

    <div class="col">
      <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
    <pngx-input-select><InputSelect title={t('common.matching_algorithm')} items={matchingAlgorithms()} bind:value={dlg.form.matching_algorithm} /></pngx-input-select>
    {#if patternRequired(dlg.form.matching_algorithm)}
      <pngx-input-text><InputText title={t('common.matching_pattern')} bind:value={dlg.form.match} error={dlg.error?.match} /></pngx-input-text>
      <pngx-input-check><InputCheck title={t('common.case_insensitive')} bind:value={dlg.form.is_insensitive} /></pngx-input-check>
    {/if}
    </div>

    {#if dlg.userIsOwner}
      <div>
        <pngx-permissions-form accordion="true"><PermissionsForm {users} accordion={true} bind:value={dlg.form.permissions_form} /></pngx-permissions-form>
      </div>
    {/if}

  </div>
  <div class="modal-footer">
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-document-type-edit-dialog>
