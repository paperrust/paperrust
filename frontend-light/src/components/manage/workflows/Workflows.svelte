<!-- Port of manage/workflows/workflows.component.html (Route workflows). Host <pngx-workflows> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/workflows.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { ricon } from '../../../lib/route-icons'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { bodyContainer } from '../../../lib/body-container'
  import { modals } from '../../../lib/modal.svelte'
  import { session } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { deleteOf, listAllOf, patchOf } from '../../../api/admin'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import WorkflowEditDialog from '../../common/edit-dialog/workflow-edit-dialog/WorkflowEditDialog.svelte'
  import { WORKFLOW_TYPE_OPTIONS, copyWorkflow } from '../../common/edit-dialog/workflow-edit-dialog/workflow-edit'
  import '../../common/edit-dialog/workflow-edit-dialog/workflow-icons'

  let workflows = $state<any[]>([])
  let loading = $state(true)
  let show = $state(false)

  async function reload() {
    loading = true
    const r = await listAllOf<any>('workflows')
    workflows = r.results
    show = true
    loading = false
  }
  reload()

  const getTypesList = (w: any) => w.triggers.map((tr: any) => t(WORKFLOW_TYPE_OPTIONS.find((x) => x.id === tr.type)!.name)).join(', ')

  function editWorkflow(workflow: any = null, forceCreate = false) {
    modals.open(
      WorkflowEditDialog,
      {
        dialogMode: workflow && !forceCreate ? 'edit' : 'create',
        // Copy so that the original stays unchanged
        object: workflow ? { ...workflow, actions: [...workflow.actions], triggers: [...workflow.triggers] } : undefined,
        onsucceeded: (n: any) => {
          toastService.showSuccess(t('manage.workflows.saved_workflow', { PH: n.name }))
          reload()
        },
        onfailed: (e: any) => toastService.showError(t('manage.workflows.error_saving_workflow'), e),
      },
      { backdrop: 'static', size: 'xl' },
    )
  }

  function deleteWorkflow(workflow: any) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('manage.workflows.confirm_delete_workflow'),
        messageBold: t('manage.workflows.operation_permanently_delete_workflow'),
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await deleteOf('workflows', workflow.id)
            ref.close()
            toastService.showSuccess(t('manage.workflows.deleted_workflow', { PH: workflow.name }))
            reload()
          } catch (e) {
            toastService.showError(t('manage.workflows.error_deleting_workflow', { PH: workflow.name }), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  async function toggleWorkflowEnabled(workflow: any) {
    try {
      await patchOf('workflows', workflow)
      toastService.showSuccess(workflow.enabled ? t('manage.workflows.enabled_workflow', { PH: workflow.name }) : t('manage.workflows.disabled_workflow', { PH: workflow.name }))
      reload()
    } catch (e) {
      toastService.showError(t('manage.workflows.error_toggling_workflow', { PH: workflow.name }), e)
    }
  }

  const menus = new Map<number, Dropdown>()
  const menu = (id: number) => {
    let d = menus.get(id)
    if (!d) menus.set(id, (d = new Dropdown()))
    return d
  }
</script>

<pngx-page-header>
<PageHeader
  title={t('common.workflows')}
  info={t("manage.workflows.use_workflows_customize_behavior")}
  infoLink="usage/#workflows"
  >
  {#if session.can('add', 'workflow')}
    <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => editWorkflow()}>
      <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('manage.workflows.add_workflow')}
    </button>
  {/if}
</PageHeader>
</pngx-page-header>

<ul class="list-group">

  <li class="list-group-item">
    <div class="row">
      <div class="col">{t('common.name')}</div>
      <div class="col d-none d-sm-flex">{t('common.sort_order')}</div>
      <div class="col">{t('common.status')}</div>
      <div class="col d-none d-sm-flex">{t('common.triggers')}</div>
      <div class="col">{t('common.actions')}</div>
    </div>
  </li>

  {#if loading && workflows.length === 0}
    <li class="list-group-item">
      <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
    </li>
  {/if}

  {#each workflows as workflow (workflow.id)}
    {@const dd = menu(workflow.id)}
    <li class="list-group-item">
      <div class="row fade" class:show>
        <div class="col d-flex align-items-center"><button class="btn btn-link p-0 text-start" type="button" onclick={() => editWorkflow(workflow)} disabled={!session.can('change', 'workflow')}>{workflow.name}</button></div>
        <div class="col d-flex align-items-center d-none d-sm-flex"><code>{workflow.order}</code></div>
        <div class="col d-flex align-items-center">
          <div class="form-check form-switch mb-0">
            {#if session.can('change', 'workflow')}
              <input type="checkbox" class="form-check-input cursor-pointer" id={workflow.id + '_enable'} bind:checked={workflow.enabled} onchange={() => toggleWorkflowEnabled(workflow)}>
            {/if}
            <label class="form-check-label cursor-pointer" for={workflow.id + '_enable'}>
              <code>{#if workflow.enabled}{t('common.enabled')}{:else}<span class="text-muted">{t('common.disabled')}</span>{/if}</code>
            </label>
          </div>
        </div>
        <div class="col d-flex align-items-center d-none d-sm-flex">{getTypesList(workflow)}</div>
        <div class="col">

          <div class="btn-group d-block d-sm-none">
            <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
              <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
                <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
              </button>
              {#if dd.open}
                <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:bodyContainer use:dd.menuEl>
                  {#if session.can('change', 'workflow')}
                    <button onclick={() => editWorkflow(workflow)} class="dropdown-item">{t('common.edit')}</button>
                  {/if}
                  {#if session.can('delete', 'workflow')}
                    <button onclick={() => deleteWorkflow(workflow)} class="dropdown-item">{t('common.delete')}</button>
                  {/if}
                  {#if session.can('add', 'workflow')}
                    <button onclick={() => editWorkflow(copyWorkflow(workflow), true)} class="dropdown-item">{t('common.copy')}</button>
                  {/if}
                </div>
              {/if}
            </div>
          </div>
          <div class="btn-toolbar d-none d-sm-flex gap-2" role="toolbar">
            <div class="btn-group">
              {#if session.can('change', 'workflow')}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editWorkflow(workflow)}>
                  <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                </button>
              {/if}
              {#if session.can('delete', 'workflow')}
                <button class="btn btn-sm btn-outline-danger" type="button" onclick={() => deleteWorkflow(workflow)}>
                  <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                </button>
              {/if}
            </div>
            <div class="btn-group">
              {#if session.can('add', 'workflow')}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editWorkflow(copyWorkflow(workflow), true)}>
                  <pngx-bs-icon width="1em" height="1em" name="files" class="me-1">{@html ricon('files', '1em', '1em')}</pngx-bs-icon>{t('common.copy')}
                </button>
              {/if}
            </div>
          </div>
        </div>
      </div>
    </li>
  {/each}
  {#if !loading && workflows.length === 0}
    <li class="list-group-item" class:show>{t('manage.workflows.no_workflows_defined')}</li>
  {/if}
</ul>
