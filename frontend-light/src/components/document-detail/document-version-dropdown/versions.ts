// Logic of document-detail/document-version-dropdown/document-version-dropdown.component.ts (framework-free, testable).

/** UploadState (services/websocket-status.service.ts), as far as the version list uses it */
export enum UploadState {
  Idle = 'idle',
  Uploading = 'uploading',
  Processing = 'processing',
  Failed = 'failed',
}

/** Response of update_version: task ID as string or { task_id } */
export function taskIdFrom(body: unknown): string | undefined {
  if (typeof body === 'string') return body || undefined
  const id = (body as { task_id?: unknown } | null)?.task_id
  return typeof id === 'string' && id ? id : undefined
}

/**
 * submitEditedVersionLabel: new value (trimmed, empty → null) or `undefined` if unchanged
 * (then only editing is ended).
 */
export function nextVersionLabel(draft: string | null | undefined, current: string | null | undefined): string | null | undefined {
  const next = draft?.trim() || null
  const cur = current?.trim() || null
  return next === cur ? undefined : next
}
