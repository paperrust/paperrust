<!-- Port of common/edit-dialog/mail-rule-edit-dialog/mail-rule-edit-dialog.component.html (size xl). -->
<script lang="ts">
  import { t, tRich } from '../../../../lib/i18n'
  import { listAllOf } from '../../../../api/admin'
  import { data } from '../../../../stores/data.svelte'
  import { toastService } from '../../../../stores/toasts.svelte'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import InputCheck from '../../input/check/InputCheck.svelte'
  import InputNumber from '../../input/number/InputNumber.svelte'
  import InputSwitch from '../../input/switch/InputSwitch.svelte'
  import InputTags from '../../input/tags/InputTags.svelte'
  import {
    ACTION_OPTIONS,
    ATTACHMENT_TYPE_OPTIONS,
    CONSUMPTION_SCOPE_OPTIONS,
    METADATA_CORRESPONDENT_OPTIONS,
    METADATA_TITLE_OPTIONS,
    PDF_LAYOUT_OPTIONS,
    MailAction,
    MailMetadataCorrespondentOption,
    showActionParamField,
    showCorrespondentField,
    translated,
  } from '../../../manage/mail/mail-options'

  let props: EditDialogProps<any> = $props()
  const dlg = new EditDialogState<any>('mail_rules' as any, props, {
    name: null,
    account: null,
    enabled: true,
    folder: 'INBOX',
    filter_from: null,
    filter_to: null,
    filter_subject: null,
    filter_body: null,
    filter_attachment_filename_include: null,
    filter_attachment_filename_exclude: null,
    maximum_age: 30, // Model default (paperless_mail.MailRule.maximum_age default=30); upstream form: null → 400 (A-231)
    attachment_type: 1,
    pdf_layout: 0,
    consumption_scope: 1,
    order: 0, // Model default (MailRule.order default=0)
    action: MailAction.MarkRead,
    action_parameter: null,
    assign_title_from: 1,
    assign_tags: [],
    assign_document_type: null,
    assign_correspondent_from: MailMetadataCorrespondentOption.FromNothing,
    assign_correspondent: null,
    assign_owner_from_rule: true,
    stop_processing: false,
  })
  const title = $derived(dlg.mode === 'create' ? t('dialog.mail_rule.create_new_mail_rule') : t('dialog.mail_rule.edit_mail_rule'))

  let accounts = $state<any[] | undefined>(undefined)
  let correspondents = $state<any[] | undefined>(undefined)
  let documentTypes = $state<any[] | undefined>(undefined)
  const load = (ep: string, msgText: string, set: (r: any[]) => void) =>
    listAllOf<any>(ep)
      .then((r) => set(r.results))
      .catch((e) => {
        toastService.showError(t(msgText), e)
        set([])
      })
  load('mail_accounts', 'common.error_retrieving_mail_accounts', (r) => (accounts = r))
  load('correspondents', 'dialog.mail_rule.error_retrieving_correspondents', (r) => (correspondents = r))
  load('document_types', 'dialog.mail_rule.error_retrieving_document_types', (r) => (documentTypes = r))
  const tags = $derived([...data.tags.values()])

  const consumptionScopeOptions = translated(CONSUMPTION_SCOPE_OPTIONS, t)
  const attachmentTypeOptions = translated(ATTACHMENT_TYPE_OPTIONS, t)
  const pdfLayoutOptions = translated(PDF_LAYOUT_OPTIONS, t)
  const actionOptions = translated(ACTION_OPTIONS, t)
  const metadataTitleOptions = translated(METADATA_TITLE_OPTIONS, t)
  const metadataCorrespondentOptions = translated(METADATA_CORRESPONDENT_OPTIONS, t)
</script>

<pngx-mail-rule-edit-dialog><form onsubmit={(e) => { e.preventDefault(); dlg.save() }} autocomplete="off">
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
      <div class="col-md-6">
        <pngx-input-text><InputText horizontal={true} title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
      </div>
      <div class="col-md-4">
        <pngx-input-select><InputSelect horizontal={true} title={t('common.account')} items={accounts} bind:value={dlg.form.account} error={dlg.error?.account} /></pngx-input-select>
      </div>
      <div class="col-md-2 pt-2">
        <pngx-input-switch><InputSwitch horizontal={true} title={t('common.enabled')} bind:value={dlg.form.enabled} /></pngx-input-switch>
      </div>
    </div>
    <div class="row">
      <div class="col-md-6">
        <pngx-input-number><InputNumber horizontal={true} title={t('dialog.mail_rule.order')} bind:value={dlg.form.order} showAdd={false} error={dlg.error?.order} /></pngx-input-number>
      </div>
      <div class="col-md-6">
        <pngx-input-switch><InputSwitch horizontal={true} title={t('dialog.mail_rule.stop_further_processing')} bind:value={dlg.form.stop_processing} hint={t('dialog.mail_rule.stop_processing_further_rules')} /></pngx-input-switch>
      </div>
    </div>
    <hr class="mt-0"/>
    <div class="row">
      <p class="small">{#each tRich('dialog.mail_rule.paperrust_only_process_mails') as seg}{#if seg.tag}<em>{seg.text}</em>{:else}{seg.text}{/if}{/each}</p>
      <div class="col-md-6">
        <pngx-input-text><InputText horizontal={true} title={t('common.folder')} bind:value={dlg.form.folder} hint={t("dialog.mail_rule.subfolders_must_separated_delimiter")} error={dlg.error?.folder} /></pngx-input-text>
        <pngx-input-number><InputNumber horizontal={true} title={t('dialog.mail_rule.maximum_age_days')} bind:value={dlg.form.maximum_age} showAdd={false} error={dlg.error?.maximum_age} /></pngx-input-number>
      </div>
      <div class="col-md-6">
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.filter')} bind:value={dlg.form.filter_from} error={dlg.error?.filter_from} /></pngx-input-text>
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.filter_2')} bind:value={dlg.form.filter_to} error={dlg.error?.filter_to} /></pngx-input-text>
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.filter_subject')} bind:value={dlg.form.filter_subject} error={dlg.error?.filter_subject} /></pngx-input-text>
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.filter_body')} bind:value={dlg.form.filter_body} error={dlg.error?.filter_body} /></pngx-input-text>
      </div>
    </div>
    <hr class="mt-0"/>
    <div class="row">
      <div class="col-md-6">
        <pngx-input-select><InputSelect horizontal={true} title={t('dialog.mail_rule.consumption_scope')} items={consumptionScopeOptions} bind:value={dlg.form.consumption_scope} hint={t('dialog.mail_rule.see_docs_eml_processing')} error={dlg.error?.consumption_scope} /></pngx-input-select>
        <pngx-input-select><InputSelect horizontal={true} title={t('dialog.mail_rule.attachment_type')} items={attachmentTypeOptions} bind:value={dlg.form.attachment_type} error={dlg.error?.attachment_type} /></pngx-input-select>
        <pngx-input-select><InputSelect horizontal={true} title={t('dialog.mail_rule.pdf_layout')} items={pdfLayoutOptions} bind:value={dlg.form.pdf_layout} error={dlg.error?.pdf_layout} /></pngx-input-select>
      </div>
      <div class="col-md-6">
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.include_only_files_matching')} bind:value={dlg.form.filter_attachment_filename_include} hint={t('dialog.mail_rule.optional_wildcards_e_g')} error={dlg.error?.filter_attachment_filename_include} /></pngx-input-text>
        <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.exclude_files_matching')} bind:value={dlg.form.filter_attachment_filename_exclude} hint={t('dialog.mail_rule.optional_wildcards_e_g')} error={dlg.error?.filter_attachment_filename_exclude} /></pngx-input-text>
      </div>
    </div>
    <hr class="mt-0"/>
    <div class="row">
      <div class="col-md-6">
        <pngx-input-select><InputSelect horizontal={true} title={t('common.action')} items={actionOptions} bind:value={dlg.form.action} hint={t('dialog.mail_rule.only_performed_mail_processed')} error={dlg.error?.action} /></pngx-input-select>
        {#if showActionParamField(dlg.form.action)}
          <pngx-input-text><InputText horizontal={true} title={t('dialog.mail_rule.action_parameter')} bind:value={dlg.form.action_parameter} error={dlg.error?.action_parameter} /></pngx-input-text>
        {/if}
        <pngx-input-select><InputSelect horizontal={true} title={t('dialog.mail_rule.assign_title')} items={metadataTitleOptions} bind:value={dlg.form.assign_title_from} error={dlg.error?.assign_title_from} /></pngx-input-select>
        <pngx-input-check><InputCheck horizontal={true} title={t('dialog.mail_rule.assign_owner_rule')} bind:value={dlg.form.assign_owner_from_rule} /></pngx-input-check>
      </div>
      <div class="col-md-6">
        <pngx-input-tags><InputTags horizontal={true} allowCreate={false} {tags} bind:value={dlg.form.assign_tags} /></pngx-input-tags>
        <pngx-input-select><InputSelect horizontal={true} title={t('common.assign_document_type')} items={documentTypes} allowNull={true} bind:value={dlg.form.assign_document_type} error={dlg.error?.assign_document_type} /></pngx-input-select>
        <pngx-input-select><InputSelect horizontal={true} title={t('dialog.mail_rule.assign_correspondent')} items={metadataCorrespondentOptions} bind:value={dlg.form.assign_correspondent_from} error={dlg.error?.assign_correspondent_from} /></pngx-input-select>
        {#if showCorrespondentField(dlg.form.assign_correspondent_from)}
          <pngx-input-select><InputSelect horizontal={true} title={t('common.assign_correspondent')} items={correspondents} allowNull={true} bind:value={dlg.form.assign_correspondent} error={dlg.error?.assign_correspondent} /></pngx-input-select>
        {/if}
      </div>
    </div>
  </div>
  <div class="modal-footer">
    {#if dlg.error?.non_field_errors}
      <span class="text-danger">{t('common.error')}: {dlg.error.non_field_errors}</span>
    {/if}
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-mail-rule-edit-dialog>
