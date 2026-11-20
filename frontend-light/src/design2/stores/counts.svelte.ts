// Counters of the sidebar (inbox, all documents) – one query per counter (page_size=1).
import { api } from '../../api/client'
import { queryString } from '../../api/v1'
import { session } from '../../stores/settings.svelte'

class Counts {
  inbox = $state<number | null>(null)
  all = $state<number | null>(null)
  async refresh() {
    if (!session.can('view', 'document')) return
    const q = (extra: Record<string, string | number>) =>
      api
        .get<{ count: number }>(`/api/documents/${queryString({ page: 1, page_size: 1, fields: 'id', ...extra })}`)
        .then((r) => r.count)
        .catch(() => null)
    const [inbox, all] = await Promise.all([q({ is_in_inbox: 'true' }), q({})])
    this.inbox = inbox
    this.all = all
  }
}
export const counts = new Counts()
