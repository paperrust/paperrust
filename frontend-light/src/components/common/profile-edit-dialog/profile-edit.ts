// Framework-free logic of ProfileEditDialogComponent (common/profile-edit-dialog/profile-edit-dialog.component.ts).
import type { UserProfile } from '../../../api/admin'

export interface ProfileForm {
  email: string | null
  email_confirm: string | null
  password: string | null
  password_confirm: string | null
  first_name: string | null
  last_name: string | null
  auth_token: string | null
  totp_code: string | null
}

/** onEmailChange/onPasswordChange: error as long as the visible confirmation field does not match */
export function confirmMismatch(shown: boolean, value: string | null | undefined, confirm: string | null | undefined): boolean {
  return shown && value !== confirm
}

/**
 * save(): `form.value` without `totp_code`. Disabled controls are missing in `form.value` – the confirmation fields are
 * only active as long as they are displayed. Order like the FormGroup.
 */
export function profilePayload(form: ProfileForm, emailConfirmShown: boolean, passwordConfirmShown: boolean): UserProfile & Record<string, any> {
  const out: Record<string, any> = { email: form.email }
  if (emailConfirmShown) out.email_confirm = form.email_confirm
  out.password = form.password
  if (passwordConfirmShown) out.password_confirm = form.password_confirm
  out.first_name = form.first_name
  out.last_name = form.last_name
  out.auth_token = form.auth_token
  return out
}

/** get qrSvgDataUrl() */
export function qrSvgDataUrl(svg: string | null | undefined): string | null {
  if (!svg) return null
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`
}

/**
 * Log out after password change (upstream: `${basePath}accounts/logout/?next=${basePath}accounts/login/?next=${basePath}`).
 * The login pages are at the server root path, return to the Light UI (`base`, e.g. /rs/app).
 */
export function logoutUrl(base: string): string {
  return `/accounts/logout/?next=${encodeURIComponent(`/accounts/login/?next=${base.replace(/\/$/, '')}/`)}`
}
