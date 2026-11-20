<!-- Port of admin/trash/trash.component.html (route trash). Host <pngx-trash> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/trash.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { ricon } from '../../../lib/route-icons'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { bodyContainer } from '../../../lib/body-container'
  import { modals } from '../../../lib/modal.svelte'
  import { router } from '../../../router.svelte'
  import { rawValue, session } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { api } from '../../../api/client'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import Pagination from '../../common/pagination/Pagination.svelte'
  import PreviewPopup from '../../common/preview-popup/PreviewPopup.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import { daysRemaining, emptyTrash as apiEmpty, getTrash, restoreDocuments, type TrashDoc } from './trash'
  import './trash-icons'

  let documentsInTrash = $state<TrashDoc[]>([])
  let selectedDocuments = $state(new Set<number>())
  let allToggled = $state(false)
  let page = $state(1)
  let totalDocuments = $state<number | undefined>(undefined)
  let loading = $state(true)
  let show = $state(false)
  const delay = $derived(+((rawValue(session.settings, 'trash_delay') as number) ?? 30))

  async function reload() {
    loading = true
    const r = await getTrash(page)
    documentsInTrash = r.results
    totalDocuments = r.count
    selectedDocuments = new Set()
    loading = false
    show = true
  }
  reload()

  const menus = new Map<number, Dropdown>()
  const menu = (id: number) => {
    let d = menus.get(id)
    if (!d) menus.set(id, (d = new Dropdown()))
    return d
  }

  function confirmDelete(messageBold: string, run: (ref: any) => Promise<void>) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('common.confirm_delete'),
        messageBold,
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.delete'),
        onconfirm: (setEnabled: (b: boolean) => void) => run({ ref, setEnabled }),
      },
      { backdrop: 'static' },
    )
  }

  function deleteDoc(document: TrashDoc) {
    confirmDelete(t('admin.trash.operation_permanently_delete_document'), async ({ ref, setEnabled }) => {
      setEnabled(false)
      try {
        await apiEmpty([document.id])
        toastService.show({ content: t('admin.trash.document_deleted', { PH: document.title }), delay: 5000, level: 'success' })
        ref.close()
        reload()
      } catch (err) {
        toastService.showError(t('admin.trash.error_deleting_document', { PH: document.title }), err)
        ref.close()
      }
    })
  }

  function emptyTrash(documents?: Set<number>) {
    confirmDelete(
      documents ? t('admin.trash.operation_permanently_delete_selected') : t('admin.trash.operation_permanently_delete_all'),
      async ({ ref }) => {
        try {
          await apiEmpty(documents ? Array.from(documents) : null)
          toastService.show({ content: t('admin.trash.document_s_deleted'), delay: 5000, level: 'success' })
          allToggled = false
          ref.close()
          reload()
        } catch (err) {
          toastService.showError(t('admin.trash.error_deleting_document_s'), err)
          ref.close()
        }
      },
    )
  }

  async function restore(document: TrashDoc) {
    try {
      await restoreDocuments([document.id])
      toastService.show({
        content: t('admin.trash.document_restored', { PH: document.title }),
        delay: 5000,
        level: 'success',
        actionName: t('common.open_document'),
        action: () => router.navigate(`documents/${document.id}`),
      })
      reload()
    } catch (err) {
      toastService.showError(t('admin.trash.error_restoring_document', { PH: document.title }), err)
    }
  }

  async function restoreAll(documents: Set<number> | null = null) {
    try {
      await restoreDocuments(documents ? Array.from(documents) : null)
      toastService.show({ content: t('common.document_s_restored'), delay: 5000, level: 'success' })
      allToggled = false
      reload()
    } catch (err) {
      toastService.showError(t('common.error_restoring_document_s'), err)
    }
  }

  function toggleAll(e: Event) {
    if ((e.target as HTMLInputElement).checked) selectedDocuments = new Set(documentsInTrash.map((d) => d.id))
    else clearSelection()
  }
  function toggleSelected(d: TrashDoc) {
    const s = new Set(selectedDocuments)
    if (s.has(d.id)) s.delete(d.id)
    else s.add(d.id)
    selectedDocuments = s
  }
  function clearSelection() {
    allToggled = false
    selectedDocuments = new Set()
  }
</script>

<pngx-page-header>
<PageHeader
    title={t('common.trash')}
    info={t('admin.trash.manage_trashed_documents_pending')}
    infoLink="usage/#document-trash">
    <button class="btn btn-sm btn-outline-secondary" onclick={clearSelection} hidden={selectedDocuments.size === 0}>
        <pngx-bs-icon name="x" class="me-1">{@html icon('x')}</pngx-bs-icon>{t('common.clear_selection')}
    </button>
    <button type="button" class="btn btn-sm btn-outline-primary" onclick={() => restoreAll(selectedDocuments)} disabled={selectedDocuments.size === 0}>
        <pngx-bs-icon name="arrow-counterclockwise" class="me-1">{@html ricon('arrow-counterclockwise')}</pngx-bs-icon>{t('admin.trash.restore_selected')}
    </button>
    <button type="button" class="btn btn-sm btn-outline-danger" onclick={() => emptyTrash(selectedDocuments)} disabled={selectedDocuments.size === 0}>
        <pngx-bs-icon name="trash" class="me-1">{@html icon('trash')}</pngx-bs-icon>{t('common.delete_selected')}
    </button>
    <button type="button" class="btn btn-sm btn-outline-danger" onclick={() => emptyTrash()} disabled={documentsInTrash.length === 0}>
        <pngx-bs-icon name="trash" class="me-1">{@html icon('trash')}</pngx-bs-icon>{t('common.empty_trash')}
    </button>
</PageHeader>
</pngx-page-header>

{#if (totalDocuments ?? 0) > 25}
    <div class="row mb-3">
        <ngb-pagination class="col-auto" size="sm" aria-label="Pagination" role="navigation"><Pagination pageSize={25} collectionSize={totalDocuments ?? 0} {page} maxSize={5} onpage={(p) => { page = p; reload() }} /></ngb-pagination>
    </div>
{/if}

<div class="card border table-responsive mb-3">
    <table class="table table-striped align-middle shadow-sm mb-0">
        <thead>
            <tr>
            <th scope="col">
                <div class="form-check m-0 ms-2 me-n2">
                <input type="checkbox" class="form-check-input" id="all-objects" bind:checked={allToggled} disabled={documentsInTrash.length === 0} onclick={(e) => { toggleAll(e); e.stopPropagation() }}>
                <label class="form-check-label" for="all-objects"></label>
                </div>
            </th>
            <th scope="col" class="fw-normal">{t('common.name')}</th>
            <th scope="col" class="fw-normal d-none d-sm-table-cell">{t('admin.trash.remaining')}</th>
            <th scope="col" class="fw-normal">{t('common.actions')}</th>
            </tr>
        </thead>
        <tbody>
            {#if loading}
                <tr>
                    <td colspan="5">
                    <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
                    </td>
                </tr>
            {/if}
            {#each documentsInTrash as document (document.id)}
                {@const dd = menu(document.id)}
                <tr onclick={(e) => { toggleSelected(document); e.stopPropagation() }} class="data-row fade" class:show>
                    <td>
                    <div class="form-check m-0 ms-2 me-n2">
                        <input type="checkbox" class="form-check-input" id={String(document.id)} checked={selectedDocuments.has(document.id)} onclick={(e) => { toggleSelected(document); e.stopPropagation() }}>
                        <label class="form-check-label" for={String(document.id)}></label>
                    </div>
                    </td>
                    <td scope="row">
                      {document.title}
                      <pngx-preview-popup><PreviewPopup id={document.id} linkClasses="btn btn-sm btn-link">
                        <pngx-bs-icon name="eye">{@html icon('eye')}</pngx-bs-icon>
                      </PreviewPopup></pngx-preview-popup>
                    </td>
                    <td scope="row" class="d-none d-sm-table-cell">{t('admin.trash.days', { INTERPOLATION: daysRemaining(document, delay) })}</td>
                    <td scope="row">
                    <div class="btn-group d-block d-sm-none">
                        <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
                        <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
                            <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
                        </button>
                        {#if dd.open}
                        <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:bodyContainer use:dd.menuEl>
                            <button onclick={() => restore(document)} class="dropdown-item">{t('admin.trash.restore')}</button>
                            <button onclick={() => deleteDoc(document)} class="dropdown-item">{t('common.delete')}</button>
                        </div>
                        {/if}
                        </div>
                    </div>
                    <div class="btn-group d-none d-sm-block">
                        <button class="btn btn-sm btn-outline-secondary" onclick={(e) => { restore(document); e.stopPropagation() }}>
                            <pngx-bs-icon width="1em" height="1em" name="arrow-counterclockwise" class="me-1">{@html ricon('arrow-counterclockwise', '1em', '1em')}</pngx-bs-icon>{t('admin.trash.restore')}
                        </button>
                        <button class="btn btn-sm btn-outline-danger" onclick={(e) => { deleteDoc(document); e.stopPropagation() }}>
                            <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                        </button>
                    </div>
                    </td>
                </tr>
            {/each}
        </tbody>
    </table>
</div>

{#if !loading}
    <div class="d-flex mb-2">
        <div>
            {t('admin.trash.total_documents_in_trash', { VAR_PLURAL: totalDocuments, INTERPOLATION: totalDocuments || 0 })}{#if selectedDocuments.size > 0}
                &nbsp;({selectedDocuments.size} selected)
            {/if}
        </div>
        {#if documentsInTrash.length > 20}
            <ngb-pagination class="ms-auto" size="sm" aria-label="Pagination" role="navigation"><Pagination pageSize={25} collectionSize={totalDocuments ?? 0} {page} maxSize={5} onpage={(p) => { page = p; reload() }} /></ngb-pagination>
        {/if}
    </div>
{/if}
