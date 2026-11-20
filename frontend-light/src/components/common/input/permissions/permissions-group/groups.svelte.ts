// GroupService.listAll (once per session, like the Angular cache).
import type { Group } from '../../../../../api/v1'
import { api } from '../../../../../api/client'

class Groups {
  list = $state<Group[]>([])
  private loading?: Promise<void>
  load() {
    this.loading ??= api
      .listAll<Group>('groups')
      .then((r) => void (this.list = r.results))
      .catch(() => {})
    return this.loading
  }
}
export const groups = new Groups()
