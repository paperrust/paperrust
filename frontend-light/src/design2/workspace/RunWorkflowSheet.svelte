<!-- Manual workflow run in the side sheet (PaperRust; idea from paperless-ngx dev #14704, own implementation).
     Choose workflow, optionally ignore the triggers' filters. onrun(workflowId, force) starts the run;
     the sheet stays open until the end (buttons locked) and the caller closes it.
     forceDefault = initial value of the switch (on for a single document, idea from paperless-ngx dev #14760);
     singleDocument = texts for one document instead of for a selection. -->
<script lang="ts">
  import { untrack } from 'svelte'
  import { t } from '../../lib/i18n'
  import type { ManualWorkflow } from '../../api/v1'
  import SideSheet from '../components/SideSheet.svelte'

  let {
    message,
    workflows,
    forceDefault = false,
    singleDocument = false,
    onclose,
    onrun,
  }: {
    message: string
    workflows: ManualWorkflow[]
    forceDefault?: boolean
    singleDocument?: boolean
    onclose: () => void
    onrun: (workflowId: number, force: boolean) => Promise<void>
  } = $props()
  let workflowId = $state<number | null>(null)
  let force = $state(untrack(() => forceDefault))
  let busy = $state(false)

  async function run() {
    if (workflowId === null) return
    busy = true
    try {
      await onrun(workflowId, force)
    } finally {
      busy = false
    }
  }
</script>

<SideSheet title={t('common.run_workflow_2')} onclose={() => !busy && onclose()} width="420px">
  <p>{message}</p>
  <label class="lbl" for="d2-run-workflow-select">{t('common.workflow')}</label>
  <select id="d2-run-workflow-select" class="d2-select" bind:value={workflowId} disabled={busy}>
    <option value={null} disabled>{t('common.select_workflow')}</option>
    {#each workflows as wf (wf.id)}
      <option value={wf.id}>{wf.name}</option>
    {/each}
  </select>
  <label class="d2-check force"><input type="checkbox" bind:checked={force} disabled={busy} id="d2-run-workflow-force" />{singleDocument ? t('common.ignore_trigger_filters') : t('common.run_all_selected_documents')}</label>
  <p class="hint">{singleDocument ? t('common.ignore_trigger_filters_document') : t("common.ignore_workflows_trigger_filters")}</p>
  {#snippet footer()}
    <button class="d2-btn" onclick={onclose} disabled={busy}>{t('common.cancel')}</button>
    <button class="d2-btn primary" onclick={run} disabled={busy || workflowId === null} id="d2-run-workflow-confirm">{t('common.run')}</button>
  {/snippet}
</SideSheet>

<style>
  .lbl {
    display: block;
    font-size: var(--d2-fs-sm);
    margin-bottom: 4px;
  }
  .d2-select {
    width: 100%;
  }
  .force {
    margin-top: var(--d2-s3);
  }
  .hint {
    font-size: var(--d2-fs-sm);
    color: var(--d2-text-2);
    margin: 4px 0 0;
  }
</style>
