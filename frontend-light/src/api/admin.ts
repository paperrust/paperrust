// v1 calls of the management/administration screens (saved views, email, users/groups, workflows,
// tasks, logs, trash, configuration) – same paths, parameters and order as the Angular services.
import { api } from './client'
import { listParams } from './objects'
import { ApiError, getCookie, type Paged } from './v1'

/** AbstractPaperlessService.list(page, pageSize, sortField, sortReverse, extraParams) */
export function listObjects<T>(endpoint: string, page?: number, pageSize?: number, sortField?: string | null, sortReverse?: boolean | null, extra: Record<string, any> = {}) {
  return api.get<Paged<T>>(`/api/${endpoint}/${listParams(page, pageSize, sortField, sortReverse, extra)}`)
}
/** AbstractPaperlessService.listAll (1, 100000) */
export const listAllOf = <T>(endpoint: string, extra: Record<string, any> = {}) => listObjects<T>(endpoint, 1, 100000, null, null, extra)

export const createOf = <T>(endpoint: string, o: any) => api.request<T>('POST', `/api/${endpoint}/`, o)
export const updateOf = <T>(endpoint: string, o: any) => api.request<T>('PUT', `/api/${endpoint}/${o.id}/`, o)
export const patchOf = <T>(endpoint: string, o: any) => api.request<T>('PATCH', `/api/${endpoint}/${o.id}/`, o)
export const deleteOf = (endpoint: string, id: number) => api.request<void>('DELETE', `/api/${endpoint}/${id}/`)

/** Multipart form (file upload, e.g. ConfigService.uploadFile) with CSRF header like the v1 client */
export async function uploadForm<T>(method: string, path: string, body: FormData): Promise<T> {
  const csrf = getCookie('csrftoken')
  const res = await fetch(path, {
    method,
    headers: { Accept: 'application/json; version=10', ...(csrf ? { 'X-CSRFToken': csrf } : {}) },
    credentials: 'same-origin',
    body,
  })
  if (!res.ok) {
    let detail: unknown = null
    try {
      detail = await res.json()
    } catch {}
    throw new ApiError(res.status, path, detail)
  }
  return (await res.json()) as T
}

// ---- ProfileService (services/profile.service.ts) ----------------------------------------------
export interface SocialAccount {
  id: number
  provider: string
  name: string
}
export interface SocialAccountProvider {
  name: string
  login_url: string
}
export interface UserProfile {
  email?: string
  password?: string
  first_name?: string
  last_name?: string
  auth_token?: string
  social_accounts?: SocialAccount[]
  has_usable_password?: boolean
  is_mfa_enabled?: boolean
}
export interface TotpSettings {
  url: string
  qr_svg: string
  secret: string
}
export const profileGet = () => api.get<UserProfile>('/api/profile/')
export const profileUpdate = (profile: UserProfile) => api.request<UserProfile>('PATCH', '/api/profile/', profile)
export const profileGenerateAuthToken = () => api.request<string>('POST', '/api/profile/generate_auth_token/', {})
export const profileDisconnectSocialAccount = (id: number) => api.request<number>('POST', '/api/profile/disconnect_social_account/', { id })
export const profileSocialAccountProviders = () => api.get<SocialAccountProvider[]>('/api/profile/social_account_providers/')
export const profileTotpSettings = () => api.get<TotpSettings>('/api/profile/totp/')
export const profileActivateTotp = (secret: string, code: string) =>
  api.request<{ success: boolean; recovery_codes: string[] }>('POST', '/api/profile/totp/', { secret, code })
export const profileDeactivateTotp = () => api.request<boolean>('DELETE', '/api/profile/totp/', {})

/** TasksService.run(taskType) – POST /api/tasks/run/ */
export const runTask = (taskType: string) => api.request<{ task_id: string }>('POST', '/api/tasks/run/', { task_type: taskType })
