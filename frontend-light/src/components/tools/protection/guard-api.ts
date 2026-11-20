// Ransomware protection (PaperRust extension): API /api/x-guard/… (crates/pl-guard/src/api.rs).
import { api } from '../../../api/client'

export interface GuardStatus {
  read_only: boolean
  since: string | null
  // superuser only
  reason?: string
  event_id?: number | null
  guard_enabled?: boolean
  backup_configured?: boolean
  last_backup?: string | null
}

export interface GuardEvent {
  id: number
  created: string
  status: 'open' | 'resolved'
  files: number
  summary: string
  resolved_at: string | null
  resolved_by: number | null
  note: string
}

export interface Finding {
  id: number
  created: string
  event_id: number | null
  path: string
  reason: string
  detail: string
}

export interface GuardOverview {
  state: { read_only: boolean; since: string | null; reason: string; event_id: number | null; lifted_at: string | null }
  guard: { enabled: boolean; threshold: number; window_minutes: number; scan_interval: number; sample_files: number; watch: boolean }
  results: GuardEvent[]
  recent_findings: Finding[]
}

export interface BackupRun {
  id: number
  started: string | null
  finished: string | null
  status: 'running' | 'ok' | 'error'
  trigger: string
  target: string
  snapshot: string | null
  files_total: number
  files_new: number
  bytes_total: number
  bytes_new: number
  db_bytes: number
  pruned: number
  warnings: string
  error: string
}

export interface BackupOverview {
  target: string | null
  config_error: string | null
  schedule: string
  last_success: BackupRun | null
  runs: BackupRun[]
}

export const guardStatus = () => api.get<GuardStatus>('/api/x-guard/status/')
export const guardOverview = () => api.get<GuardOverview>('/api/x-guard/events/')
export const guardEvent = (id: number) => api.get<{ event: GuardEvent; findings: Finding[] }>(`/api/x-guard/events/${id}/`)
export const liftReadOnly = (note: string) => api.request<{ state: GuardOverview['state'] }>('POST', '/api/x-guard/lift/', { note })
export const backupOverview = () => api.get<BackupOverview>('/api/x-guard/backup/')
export const runBackup = () => api.request<{ started: boolean }>('POST', '/api/x-guard/backup/run/', {})

/** Translation key per finding reason (crates/pl-guard/src/guard.rs `Reason`). */
export const REASON_KEYS: Record<string, string> = {
  modified: 'protection.reason_modified',
  encrypted: 'protection.reason_encrypted',
  ransom_note: 'protection.reason_ransom_note',
  suspicious_extension: 'protection.reason_suspicious_extension',
  deleted: 'protection.reason_deleted',
  unknown_extension: 'protection.reason_unknown_extension',
  bad_magic: 'protection.reason_bad_magic',
}

/** Bytes readable (1024 steps). */
export function formatBytes(n: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let v = n
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${i === 0 ? v : v.toFixed(1)} ${units[i]}`
}
