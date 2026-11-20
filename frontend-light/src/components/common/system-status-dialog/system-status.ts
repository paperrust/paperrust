// Framework-free parts of SystemStatusDialogComponent (common/system-status-dialog) and data/system-status.ts.

export const SystemStatusItemStatus = {
  OK: 'OK',
  ERROR: 'ERROR',
  WARNING: 'WARNING',
  DISABLED: 'DISABLED',
} as const

/** PaperlessTaskType, as far as TasksViewSet.run accepts it (pl-system tasks/run.rs `runnable`) */
export const RunnableTask = {
  TrainClassifier: 'train_classifier',
  SanityCheck: 'sanity_check',
  LlmIndex: 'llm_index',
} as const

/** SystemStatusDialogComponent.isStale */
export function isStale(dateStr: string | null | undefined, hours = 24, now: Date = new Date()): boolean {
  const date = new Date(dateStr as string)
  return now.getTime() - date.getTime() > hours * 60 * 60 * 1000
}

/** FileSizePipe (default precision: bytes/KB 0, MB/GB 1, TB/PB 2) */
export function fileSize(bytes: number | null | undefined = 0): string {
  const units = ['bytes', 'KB', 'MB', 'GB', 'TB', 'PB']
  const precision: Record<string, number> = { bytes: 0, KB: 0, MB: 1, GB: 1, TB: 2, PB: 2 }
  if (bytes == null || isNaN(parseFloat(String(bytes))) || !isFinite(bytes)) return '?'
  let i = 0
  while (bytes >= 1024) {
    bytes /= 1024
    i++
  }
  return `${bytes.toFixed(precision[units[i]])} ${units[i]}`
}

/** Text class of the warning icon when status != OK (ERROR → danger, WARNING → warning, DISABLED → muted only for the AI index) */
export function statusIconClass(status: string | null | undefined, mutedWhenDisabled = false): string {
  if (status === SystemStatusItemStatus.ERROR) return 'text-danger'
  if (status === SystemStatusItemStatus.WARNING) return 'text-warning'
  if (mutedWhenDisabled && status === SystemStatusItemStatus.DISABLED) return 'text-muted'
  return ''
}
