// Logic of common/edit-dialog/edit-dialog.component.ts (EditDialogComponent): form from object or
// default values (incl. default permissions from the UI settings), save via POST/PUT, errors per field.
import { createObject, updateObject, type ObjectEndpoint } from '../../../api/objects'
import type { ModalRef } from '../../../lib/modal.svelte'
import { session } from '../../../stores/settings.svelte'
import { EXTRA_KEYS, setting } from '../../../stores/settingsSave.svelte'

export type EditDialogMode = 'create' | 'edit'

export interface EditDialogProps<T = any> {
  modal: ModalRef<any>
  dialogMode?: EditDialogMode
  object?: T | null
  onsucceeded?: (result: T) => void
  onfailed?: (e: any) => void
}

/** FormGroup.patchValue: only take over known keys */
export function patchForm(form: Record<string, any>, value: Record<string, any> | null | undefined) {
  if (!value) return
  for (const k of Object.keys(form)) if (k in value && value[k] !== undefined) form[k] = value[k]
}

export function defaultPermissionsForm() {
  return {
    owner: setting(EXTRA_KEYS.DEFAULT_PERMS_OWNER) ?? null,
    set_permissions: {
      view: { users: setting(EXTRA_KEYS.DEFAULT_PERMS_VIEW_USERS) ?? [], groups: setting(EXTRA_KEYS.DEFAULT_PERMS_VIEW_GROUPS) ?? [] },
      change: { users: setting(EXTRA_KEYS.DEFAULT_PERMS_EDIT_USERS) ?? [], groups: setting(EXTRA_KEYS.DEFAULT_PERMS_EDIT_GROUPS) ?? [] },
    },
  }
}

export class EditDialogState<T extends { id?: number; owner?: number | null; permissions?: any } = any> {
  form = $state<Record<string, any>>({})
  error = $state<any>(null)
  networkActive = $state(false)
  closeEnabled = $state(false)
  readonly mode: EditDialogMode
  readonly object: T | null

  constructor(
    private endpoint: ObjectEndpoint,
    private props: EditDialogProps<T>,
    defaults: Record<string, any>,
  ) {
    this.mode = props.dialogMode ?? 'create'
    this.object = props.object ?? null
    const form = { ...defaults }
    if (this.object != null && this.mode !== 'create') {
      patchForm(form, { ...this.object, permissions_form: { owner: this.object.owner, set_permissions: this.object.permissions } })
    } else {
      patchForm(form, this.object as any)
      if ('permissions_form' in form) form.permissions_form = defaultPermissionsForm()
    }
    this.form = form
    // Enable the close button only after the first render (focus stays in the input field)
    setTimeout(() => (this.closeEnabled = true))
  }

  /** shouldSubmitPermissions */
  get shouldSubmitPermissions() {
    return this.mode === 'create' || session.ownsObject(this.object as any)
  }

  /** IfOwnerDirective */
  get userIsOwner() {
    return session.ownsObject(this.object as any)
  }

  async save(getFormValues: () => Record<string, any> = () => ({ ...$state.snapshot(this.form) })) {
    this.error = null
    const formValues = getFormValues()
    const perms = formValues.permissions_form
    if (perms && this.shouldSubmitPermissions) {
      formValues.owner = perms.owner
      formValues.set_permissions = perms.set_permissions
    }
    delete formValues.permissions_form
    const newObject: any = { ...(this.object ?? {}), ...formValues }
    if (!this.shouldSubmitPermissions) delete newObject['set_permissions']
    this.networkActive = true
    try {
      const result = this.mode === 'create' ? await createObject<T>(this.endpoint, newObject) : await updateObject<T>(this.endpoint, newObject)
      this.props.modal.close()
      this.props.onsucceeded?.(result)
    } catch (e: any) {
      this.error = e?.body ?? null
      this.networkActive = false
      this.props.onfailed?.(e)
    }
  }

  cancel() {
    this.props.modal.close()
  }
}
