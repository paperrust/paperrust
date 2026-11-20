import { describe, expect, it } from 'vitest'
import { confirmMismatch, logoutUrl, profilePayload, qrSvgDataUrl, type ProfileForm } from './profile-edit'

const form: ProfileForm = {
  email: 'new@example.com',
  email_confirm: 'new@example.com',
  password: 'secret',
  password_confirm: 'other',
  first_name: 'A',
  last_name: 'B',
  auth_token: 'tok',
  totp_code: '123456',
}

describe('ProfileEditDialog logic', () => {
  it('confirm fields only error while shown and different', () => {
    expect(confirmMismatch(false, 'a', 'b')).toBe(false)
    expect(confirmMismatch(true, 'a', 'a')).toBe(false)
    expect(confirmMismatch(true, 'a', null)).toBe(true)
    expect(confirmMismatch(true, 'a', 'b')).toBe(true)
  })
  it('payload = form.value without totp_code and disabled confirm controls, in FormGroup order', () => {
    expect(Object.keys(profilePayload(form, false, false))).toEqual(['email', 'password', 'first_name', 'last_name', 'auth_token'])
    const p = profilePayload(form, true, true)
    expect(Object.keys(p)).toEqual(['email', 'email_confirm', 'password', 'password_confirm', 'first_name', 'last_name', 'auth_token'])
    expect(p.totp_code).toBeUndefined()
  })
  it('qr svg data url', () => {
    expect(qrSvgDataUrl(undefined)).toBeNull()
    expect(qrSvgDataUrl('<svg a="1"/>')).toBe('data:image/svg+xml;utf8,%3Csvg%20a%3D%221%22%2F%3E')
  })
  it('logout url returns to the light UI after login', () => {
    expect(logoutUrl('/rs/app')).toBe('/accounts/logout/?next=%2Faccounts%2Flogin%2F%3Fnext%3D%2Frs%2Fapp%2F')
    expect(logoutUrl('/rs/app/')).toBe(logoutUrl('/rs/app'))
  })
})
