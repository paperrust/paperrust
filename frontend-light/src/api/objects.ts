// v1 CRUD for master data (tags, correspondents, document types, storage paths, custom fields) like
// AbstractPaperlessService / AbstractNameFilterService (same parameters in the same order).
import { api } from './client'
import type { Paged } from './v1'

export type ObjectEndpoint = 'tags' | 'correspondents' | 'document_types' | 'storage_paths' | 'custom_fields'

export interface ObjectPermissionsPayload {
  owner: number | null
  set_permissions: { view: { users: number[]; groups: number[] }; change: { users: number[]; groups: number[] } }
}

/** HttpParams: omit null/undefined, keep the order of the keys */
export function listParams(page?: number, pageSize?: number, sortField?: string | null, sortReverse?: boolean | null, extra: Record<string, any> = {}): string {
  const p: [string, string][] = []
  if (page) p.push(['page', String(page)])
  if (pageSize) p.push(['page_size', String(pageSize)])
  if (sortField) p.push(['ordering', (sortReverse ? '-' : '') + sortField])
  for (const [k, v] of Object.entries(extra)) if (v != null) p.push([k, String(v)])
  return p.length ? '?' + p.map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`).join('&') : ''
}

/** AbstractNameFilterService.listFiltered (extraParams first, then name__icontains, full_perms) */
export function listFiltered<T>(
  endpoint: ObjectEndpoint,
  page: number,
  pageSize: number,
  sortField: string | null | undefined,
  sortReverse: boolean | null | undefined,
  nameFilter: string | null | undefined,
  fullPerms: boolean,
  extraParams: Record<string, any> | null,
) {
  const params: Record<string, any> = { ...(extraParams ?? {}) }
  if (nameFilter) params['name__icontains'] = nameFilter
  if (fullPerms) params['full_perms'] = true
  return api.get<Paged<T> & { display_count?: number }>(`/api/${endpoint}/${listParams(page, pageSize, sortField ?? undefined, sortReverse ?? undefined, params)}`)
}

export function listAllObjects<T>(endpoint: ObjectEndpoint) {
  return api.get<Paged<T>>(`/api/${endpoint}/${listParams(1, 100000)}`)
}

export function createObject<T>(endpoint: ObjectEndpoint, o: any) {
  return api.request<T>('POST', `/api/${endpoint}/`, o)
}

export function updateObject<T>(endpoint: ObjectEndpoint, o: any) {
  return api.request<T>('PUT', `/api/${endpoint}/${o.id}/`, o)
}

export function deleteObject(endpoint: ObjectEndpoint, id: number) {
  return api.request<void>('DELETE', `/api/${endpoint}/${id}/`)
}

/** AbstractNameFilterService.bulk_edit_objects */
export function bulkEditObjects(
  endpoint: ObjectEndpoint,
  objects: number[],
  operation: 'set_permissions' | 'delete',
  permissions: ObjectPermissionsPayload | null = null,
  merge: boolean | null = null,
  all = false,
  filters: Record<string, any> | null = null,
) {
  const params: any = { object_type: endpoint, operation }
  if (all) {
    params['all'] = true
    if (filters) params['filters'] = filters
  } else params['objects'] = objects
  if (operation === 'set_permissions') {
    params['owner'] = permissions?.owner
    params['permissions'] = permissions?.set_permissions
    params['merge'] = merge
  }
  return api.request<string>('POST', '/api/bulk_edit_objects/', params)
}

/** StoragePathService.testPath */
export function testStoragePath(path: string, document: number) {
  return api.request<string>('POST', '/api/storage_paths/test/', { path, document })
}

/** SettingsService.storeSettings */
export function storeUiSettings(settings: Record<string, any>) {
  return api.request<{ success?: boolean }>('POST', '/api/ui_settings/', { settings })
}

/** SystemStatusService.get */
export function systemStatus() {
  return api.get<any>('/api/status/')
}
