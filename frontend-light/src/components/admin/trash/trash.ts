// TrashService (getTrash/emptyTrash/restoreDocuments) + TrashComponent.getDaysRemaining.
import { api } from '../../../api/client'
import type { Paged } from '../../../api/v1'

export interface TrashDoc {
  id: number
  title: string
  deleted_at: string
}

export const getTrash = (page = 1) => api.get<Paged<TrashDoc>>(`/api/trash/?page=${page}`)

export function emptyTrash(documents?: number[] | null) {
  const data: Record<string, any> = { action: 'empty' }
  if (documents?.length) data['documents'] = documents
  return api.request('POST', '/api/trash/', data)
}

export const restoreDocuments = (documents: number[] | null) => api.request('POST', '/api/trash/', { action: 'restore', documents })

/**
 * Delay (UI setting trash_delay, default 30) minus started days since deletion; never negative
 * (expired entries only wait for the next automatic emptying → 0, test log A-131).
 * Upstream (TrashComponent.getDaysRemaining) shows negative values here.
 */
export function daysRemaining(document: { deleted_at: string }, delay: number, now = new Date()): number {
  const diff = now.getTime() - new Date(document.deleted_at).getTime()
  const left = delay - Math.ceil(diff / (1000 * 3600 * 24))
  return Number.isFinite(left) ? Math.max(0, left) : 0
}
