// AI status for navigation and home page (GET /api/x-ai/status/): AI on?, open suggestions. One fetch at start,
// then after every decision in the review list and after ingestion. Without an AI part in the server (404) everything stays off.
import { getStatus, type AiStatus } from './api'

class AiStatusStore {
  data = $state<AiStatus | null>(null)
  get enabled() {
    return !!this.data?.enabled
  }
  get pending() {
    return this.data?.pending ?? null
  }
  /** AI text recognition selectable (AI on and option "text recognition" on) – switch on upload/reprocess */
  get vision() {
    return !!(this.data?.enabled && this.data?.vision_enabled)
  }
  private loading: Promise<void> | null = null
  async refresh() {
    try {
      this.data = await getStatus()
    } catch {
      this.data = null
    }
  }
  /** load once if not done yet (the classic UI never fetches the status otherwise) */
  ensure() {
    if (!this.data && !this.loading) this.loading = this.refresh().finally(() => (this.loading = null))
  }
}
export const aiStatus = new AiStatusStore()
