// Classic dialogs in the workspace of the PaperRust design (feedback 2026-11-18: PDF tools were missing).
// A single output location per page: Workspace.svelte shows `classicDialogs.host` as soon as an action needs it.
// Administration/settings have their own ModalHost (ManageHost/SettingsHost) and are other routes.
import type { Component } from 'svelte'

export const classicDialogs = $state<{ host: Component | null }>({ host: null })

let loading: Promise<void> | null = null

/** Loads style + output location of the classic dialogs once */
export function ensureClassicDialogs(): Promise<void> {
  loading ??= import('./ClassicDialogs.svelte').then((m) => {
    classicDialogs.host = m.default
  })
  return loading
}
