// Dialog actions of ManagementListComponent (create, edit, delete, bulk permissions, bulk delete,
// save page size) – same texts, toasts and API calls as Angular.
import type { Component } from 'svelte'
import { t } from '../../../../lib/i18n'
import { modals } from '../../../../lib/modal.svelte'
import { toastService } from '../../../../stores/toasts.svelte'
import { bulkEditObjects, deleteObject, listFiltered } from '../../../../api/objects'
import { EXTRA_KEYS, setSetting, setting, storeSettings } from '../../../../stores/settingsSave.svelte'
import { session } from '../../../../stores/settings.svelte'
import ConfirmDialog from '../../../common/confirm-dialog/ConfirmDialog.svelte'
import PermissionsDialog from '../../../common/permissions-dialog/PermissionsDialog.svelte'
import TagEditDialog from '../../../common/edit-dialog/tag-edit-dialog/TagEditDialog.svelte'
import CorrespondentEditDialog from '../../../common/edit-dialog/correspondent-edit-dialog/CorrespondentEditDialog.svelte'
import DocumentTypeEditDialog from '../../../common/edit-dialog/document-type-edit-dialog/DocumentTypeEditDialog.svelte'
import StoragePathEditDialog from '../../../common/edit-dialog/storage-path-edit-dialog/StoragePathEditDialog.svelte'
import { LIST_CONFIGS, ManagementListState, type ListKind } from './management-list.svelte'
import type { MatchingModel } from '../../../../lib/matching'

const EDIT_DIALOGS: Record<ListKind, Component<any>> = {
  tags: TagEditDialog,
  correspondents: CorrespondentEditDialog,
  documenttypes: DocumentTypeEditDialog,
  storagepaths: StoragePathEditDialog,
}

/** pageSize getter: key is the (translated) typeNamePlural, like Angular */
export function pageSizeFor(kind: ListKind): number {
  return setting(EXTRA_KEYS.OBJECT_LIST_SIZES)?.[t(LIST_CONFIGS[kind].typeNamePlural)] || 25
}

export function createListState(kind: ListKind) {
  const c = LIST_CONFIGS[kind]
  return new ManagementListState(c, (...a) => listFiltered<MatchingModel>(c.endpoint, ...a), () => pageSizeFor(kind))
}

/** pageSize setter: save UI settings, then reload page 1 */
export async function setPageSize(list: ManagementListState, size: number) {
  setSetting(EXTRA_KEYS.OBJECT_LIST_SIZES, { ...setting(EXTRA_KEYS.OBJECT_LIST_SIZES), [t(list.config.typeNamePlural)]: size })
  try {
    await storeSettings()
    list.page = 1
    list.reloadData()
  } catch (error) {
    toastService.showError(t('manage.attributes.error_saving_settings'), error)
  }
}

export function openCreateDialog(list: ManagementListState) {
  const typeName = t(list.config.typeName)
  modals.open(
    EDIT_DIALOGS[list.config.kind],
    {
      dialogMode: 'create',
      onsucceeded: () => {
        list.reloadData()
        toastService.showSuccess(t('manage.attributes.successfully_created', { PH: typeName }))
      },
      onfailed: (e: any) => toastService.showError(t('manage.attributes.error_occurred_while_creating', { PH: typeName }), e),
    },
    { backdrop: 'static' },
  )
}

export function openEditDialog(list: ManagementListState, object: MatchingModel) {
  const typeName = t(list.config.typeName)
  modals.open(
    EDIT_DIALOGS[list.config.kind],
    {
      object,
      dialogMode: 'edit',
      onsucceeded: () => {
        list.reloadData()
        toastService.showSuccess(t('manage.attributes.successfully_updated', { PH: typeName, PH_1: object.name }))
      },
      onfailed: (e: any) => toastService.showError(t('manage.attributes.error_occurred_while_saving', { PH: typeName }), e),
    },
    { backdrop: 'static' },
  )
}

export function openDeleteDialog(list: ManagementListState, object: MatchingModel) {
  const ref = modals.open<string>(
    ConfirmDialog,
    {
      title: t('common.confirm_delete'),
      messageBold: t(list.config.deleteMessage, { PH: object.name }),
      message: t('manage.attributes.associated_documents_not_deleted'),
      btnClass: 'btn-danger',
      btnCaption: t('common.delete'),
      onconfirm: async (setEnabled: (b: boolean) => void) => {
        setEnabled(false)
        try {
          await deleteObject(list.config.endpoint, object.id)
          ref.close()
          list.reloadData()
        } catch (error) {
          setEnabled(true)
          toastService.showError(t('manage.attributes.error_while_deleting_element'), error)
        }
      },
    },
    { backdrop: 'static' },
  )
}

const bulkIds = (list: ManagementListState) => (list.isAllSelection ? [] : Array.from(list.selectedObjects))

export function setPermissions(list: ManagementListState) {
  const ref = modals.open(
    PermissionsDialog,
    {
      onconfirm: async ({ permissions, merge }: any, setEnabled: (b: boolean) => void) => {
        setEnabled(false)
        try {
          await bulkEditObjects(list.config.endpoint, bulkIds(list), 'set_permissions', permissions, merge, list.isAllSelection, list.isAllSelection ? list.bulkEditFilters() : null)
          ref.close()
          toastService.showSuccess(t('manage.attributes.permissions_updated_successfully'))
          list.reloadData()
        } catch (error) {
          setEnabled(true)
          toastService.showError(t('common.error_updating_permissions'), error)
        }
      },
    },
    { backdrop: 'static' },
  )
}

export function bulkDelete(list: ManagementListState) {
  const ref = modals.open<string>(
    ConfirmDialog,
    {
      title: t('common.confirm_delete'),
      messageBold: t('manage.attributes.operation_permanently_delete_selected', { PH: t(list.config.typeNamePlural) }),
      message: t('common.operation_cannot_undone'),
      btnClass: 'btn-danger',
      btnCaption: t('common.proceed'),
      onconfirm: async (setEnabled: (b: boolean) => void) => {
        setEnabled(false)
        try {
          await bulkEditObjects(list.config.endpoint, bulkIds(list), 'delete', null, null, list.isAllSelection, list.isAllSelection ? list.bulkEditFilters() : null)
          ref.close()
          toastService.showSuccess(t('manage.attributes.objects_deleted_successfully'))
          list.reloadData()
        } catch (error) {
          setEnabled(true)
          toastService.showError(t('manage.attributes.error_deleting_objects'), error)
        }
      },
    },
    { backdrop: 'static' },
  )
}

/** userCanEdit / userCanDelete / userCanBulkEdit */
export const userCanEdit = (o: MatchingModel) => session.hasObjectPermissions('change', o)
export const userCanDelete = (o: MatchingModel) => session.ownsObject(o)
export function userCanBulkEdit(list: ManagementListState, action: 'change' | 'delete'): boolean {
  if (!session.can(action, list.config.permissionType)) return false
  return list.data.filter((o) => list.selectedObjects.has(o.id)).every((o) => session.ownsObject(o))
}
