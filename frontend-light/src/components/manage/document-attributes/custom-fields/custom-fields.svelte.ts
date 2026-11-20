// Logic of manage/document-attributes/custom-fields/custom-fields.component.ts.
import type { CustomField } from '../../../../api/v1'
import { deleteObject, listAllObjects } from '../../../../api/objects'
import { t } from '../../../../lib/i18n'
import { modals } from '../../../../lib/modal.svelte'
import { dataTypeLabels } from '../../../../lib/matching'
import { quickFilterPath } from '../../../../lib/quick-filter'
import { FILTER_CUSTOM_FIELDS_QUERY } from '../../../../lib/filter-rule-type'
import { toastService } from '../../../../stores/toasts.svelte'
import { data } from '../../../../stores/data.svelte'
import ConfirmDialog from '../../../common/confirm-dialog/ConfirmDialog.svelte'
import CustomFieldEditDialog from '../../../common/edit-dialog/custom-field-edit-dialog/CustomFieldEditDialog.svelte'

/** getDocumentFilterUrl: field present (OR [[id, exists, true]]) */
export function documentFilterPath(field: CustomField) {
  return quickFilterPath([{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: JSON.stringify(['OR', [[field.id, 'exists', true]]]) }])
}

export class CustomFieldsState {
  fields = $state<CustomField[]>([])
  loading = $state(true)
  show = $state(false)

  async reload() {
    this.loading = true
    try {
      const r = await listAllObjects<CustomField>('custom_fields')
      this.fields = r.results
      // CustomFieldsService.clearCache: also refresh the frame's/list's master data
      data.customFields = r.results
    } finally {
      this.show = true
      this.loading = false
    }
  }

  getDataType(field: CustomField): string {
    return dataTypeLabels().find((l) => l.id === field.data_type)?.name ?? ''
  }

  editField(field: CustomField | null) {
    modals.open(CustomFieldEditDialog, {
      dialogMode: field ? 'edit' : 'create',
      object: field,
      onsucceeded: (newField: CustomField) => {
        toastService.showSuccess(t('common.saved_field', { PH: newField.name }))
        this.reload()
      },
      onfailed: (e: any) => toastService.showError(t('common.error_saving_field'), e),
    })
  }

  deleteField(field: CustomField) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('manage.attributes.confirm_delete_field'),
        messageBold: t('manage.attributes.operation_permanently_delete_field'),
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await deleteObject('custom_fields', field.id)
            ref.close()
            toastService.showSuccess(t('manage.attributes.deleted_field', { PH: field.name }))
            this.reload()
          } catch (e) {
            toastService.showError(t('manage.attributes.error_deleting_field', { PH: field.name }), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
}
