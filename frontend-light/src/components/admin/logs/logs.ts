// LogService + LogsComponent.getLogLevel (framework-free).
import { api } from '../../../api/client'

export const listLogs = () => api.get<string[]>('/api/logs/')
export const getLog = (id: string, limit?: number) => api.get<string[]>(`/api/logs/${id}/${limit ? `?limit=${limit}` : ''}`)

export function getLogLevel(log: string): number {
  if (log.indexOf('[DEBUG]') != -1) return 10
  if (log.indexOf('[WARNING]') != -1) return 30
  if (log.indexOf('[ERROR]') != -1) return 40
  if (log.indexOf('[CRITICAL]') != -1) return 50
  return 20
}

export const parseLogsWithLevel = (logs: string[]) => logs.map((message) => ({ message, level: getLogLevel(message) }))

export function hasChanges(next: { message: string; level: number }[], current: { message: string; level: number }[]) {
  return next.length !== current.length || next.some((l, i) => current[i]?.message !== l.message || current[i]?.level !== l.level)
}
