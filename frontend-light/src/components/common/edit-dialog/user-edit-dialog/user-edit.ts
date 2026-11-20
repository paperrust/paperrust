// UserEditDialogComponent: objectForm.value without disabled controls (FormGroup.value semantics):
// user_permissions for superuser (onToggleSuperUser), is_superuser for non-superusers.
export function userFormValues(form: Record<string, any>, currentUserIsSuperUser: boolean): Record<string, any> {
  const v = { ...form }
  if (v.is_superuser) delete v.user_permissions
  if (!currentUserIsSuperUser) delete v.is_superuser
  return v
}

/** save(): passwordIsSet = password field contains more than asterisks */
export const passwordIsSet = (password: unknown) => (password?.toString().replaceAll('*', '').length ?? 0) > 0
