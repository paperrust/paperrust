// Load toast and upload list only when there is something to show (first call stays within budget, FRONTEND-LIGHT §1).
import type { Component } from 'svelte'

export class Lazy {
  component = $state<Component<any> | null>(null)
  private loading = false
  constructor(private loader: () => Promise<{ default: Component<any> }>) {}
  /** loads on first need; returns the component as soon as it is available */
  get(needed: boolean): Component<any> | null {
    if (needed && !this.component && !this.loading) {
      this.loading = true
      this.loader().then((m) => (this.component = m.default))
    }
    return this.component
  }
}

export const LazyToast = new Lazy(() => import('./Toast.svelte'))
export const LazyUploadStatusTray = new Lazy(() => import('../upload-status-tray/UploadStatusTray.svelte'))
