<!-- Manual workflow run (PaperRust; idea from paperless-ngx dev #14704, own implementation): choose workflow,
     optionally "apply to all selected documents" (= ignore the triggers' filters). Confirm calls
     onconfirm(setEnabled, workflowId, force); cancel = dismiss. Confirm only after a workflow is selected.
     forceDefault = initial value of the switch (on for a single document, idea from paperless-ngx dev #14760);
     singleDocument = texts for one document instead of for a selection. -->
<script lang="ts">
  import { t } from '../../../../lib/i18n'
  import { untrack } from 'svelte'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import type { ManualWorkflow } from '../../../../api/v1'
  let {
    modal,
    title = t('common.run_workflow_2'),
    message = '',
    btnClass = 'btn-primary',
    btnCaption = t('common.run'),
    cancelBtnClass = 'btn-outline-secondary',
    cancelBtnCaption = t('common.cancel'),
    workflows = [],
    forceDefault = false,
    singleDocument = false,
    onconfirm,
  }: {
    modal: ModalRef<string>
    title?: string
    message?: string
    btnClass?: string
    btnCaption?: string
    cancelBtnClass?: string
    cancelBtnCaption?: string
    workflows?: ManualWorkflow[]
    forceDefault?: boolean
    singleDocument?: boolean
    onconfirm?: (setEnabled: (b: boolean) => void, workflowId: number, force: boolean) => void | Promise<void>
  } = $props()
  let buttonsEnabled = $state(true)
  let workflowId = $state<number | null>(null)
  let force = $state(untrack(() => forceDefault))

  async function confirm() {
    if (workflowId === null) return
    if (onconfirm) await onconfirm((b) => (buttonsEnabled = b), workflowId, force)
    else modal.close('confirm')
  }
</script>

<pngx-run-workflow-confirm-dialog><div class="modal-header">
  <h4 class="modal-title" id="modal-basic-title">{title}</h4>
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.dismiss()}>
  </button>
</div>
<div class="modal-body">
  {#if message}
    <p class="text-break">{message}</p>
  {/if}
  <div class="mb-3">
    <label class="form-label" for="runWorkflowSelect">{t('common.workflow')}</label>
    <select class="form-select" id="runWorkflowSelect" bind:value={workflowId}>
      <option value={null} disabled>{t('common.select_workflow')}</option>
      {#each workflows as wf (wf.id)}
        <option value={wf.id}>{wf.name}</option>
      {/each}
    </select>
  </div>
  <div class="form-check">
    <input class="form-check-input" type="checkbox" id="runWorkflowForce" bind:checked={force} />
    {#if singleDocument}
      <label class="form-check-label" for="runWorkflowForce">{t('common.ignore_trigger_filters')}</label>
      <div class="form-text">{t('common.ignore_trigger_filters_document')}</div>
    {:else}
      <label class="form-check-label" for="runWorkflowForce">{t('common.run_all_selected_documents')}</label>
      <div class="form-text">{t("common.ignore_workflows_trigger_filters")}</div>
    {/if}
  </div>
</div>
<div class="modal-footer">
  <button type="button" class="btn {cancelBtnClass}" onclick={() => modal.dismiss()} disabled={!buttonsEnabled}>
    <span class="d-inline-block" style={"padding-bottom: 1px;"}>{cancelBtnCaption}</span>
  </button>
  <button type="button" class="btn {btnClass}" id="runWorkflowConfirm" onclick={confirm} disabled={workflowId === null || !buttonsEnabled}>
    {btnCaption}
  </button>
</div></pngx-run-workflow-confirm-dialog>
