// Folder view state: switch (ui_settings), folder list with counts, collapsed tree nodes (per browser), stage 2:
// list/tiles mode (ui_settings, per user), the item being dragged, moving documents/folders with undo.
import { t } from '../../lib/i18n'
import { session } from '../../stores/settings.svelte'
import { toastService } from '../../stores/toasts.svelte'
import { FOLDERS_KEY, foldersEnabled, loadFolders, moveDocuments, moveFolder, type Folder } from '../api/folders'
import { FOLDER_VIEW_KEY, folderViewMode, undoGroups, type DragItem, type DropKey, type FolderViewMode } from '../lib/folder-dnd'
import { ws } from './workspace.svelte'

const COLLAPSED_KEY = 'paperrust.folders.collapsed'

function readCollapsed(): Set<number> {
  try {
    const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]')
    return new Set(Array.isArray(v) ? v.filter((x) => typeof x === 'number') : [])
  } catch {
    return new Set()
  }
}

class FolderState {
  items = $state<Folder[]>([])
  unsorted = $state<number | null>(null)
  canOrganize = $state(false)
  loaded = $state(false)
  error = $state<string | null>(null)
  collapsed = $state<Set<number>>(readCollapsed())
  /** Internal drag in progress (documents or a folder); dragover cannot read the DataTransfer data */
  drag = $state<DragItem | null>(null)
  /** Drop target under the pointer (highlight) */
  dropOver = $state<DropKey | null>(null)
  /** Busy with a move (drop or bulk bar) */
  moving = $state(false)
  private seq = 0

  /** Folder view switched on for this user */
  get enabled() {
    return foldersEnabled(session.settings)
  }

  async refresh() {
    const my = ++this.seq
    try {
      const r = await loadFolders()
      if (my !== this.seq) return
      this.items = r.items
      this.unsorted = r.unsorted
      this.canOrganize = r.can_organize
      this.error = null
    } catch (e: any) {
      if (my !== this.seq) return
      this.error = e?.message ?? String(e)
    } finally {
      if (my === this.seq) this.loaded = true
    }
  }

  /** Load once (when the folder view / a folder field is shown first) */
  ensure() {
    if (!this.loaded && this.seq === 0) void this.refresh()
  }

  byId(id: number | null | undefined): Folder | undefined {
    return id === null || id === undefined ? undefined : this.items.find((f) => f.id === id)
  }

  toggle(id: number) {
    const next = new Set(this.collapsed)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    this.collapsed = next
    try {
      localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...next]))
    } catch {}
  }

  /** List or tiles in the folder view (per user, ui_settings.paperrust.folder_view) */
  get view(): FolderViewMode {
    return folderViewMode(session.settings)
  }

  async setView(mode: FolderViewMode) {
    const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
    setSetting(FOLDER_VIEW_KEY, mode)
    await storeSettings().catch(() => {})
  }

  /**
   * Move documents into `folder` (null = Unsorted) – bulk bar and drag & drop. Afterwards the selection is cleared;
   * in the folder view, documents that left the open folder leave the list. Toast with "Undo" when the previous
   * folders are known (documents of the list).
   */
  async moveDocs(ids: number[], folder: number | null, undo = true): Promise<boolean> {
    if (!ids.length || this.moving) return false
    const inFolder = ws.ctx.kind === 'folder'
    const here = inFolder ? (ws.ctx.folder === 'unsorted' || ws.ctx.folder === undefined ? null : ws.ctx.folder) : undefined
    const prev = (id: number) => {
      const d = ws.docs.find((x) => x.id === id)
      if (d && d.folder !== undefined) return d.folder
      return d && inFolder ? (here as number | null) : undefined
    }
    const groups = undo ? undoGroups(ids, folder, prev) : null
    this.moving = true
    try {
      await moveDocuments(ids, folder)
    } catch (e) {
      toastService.showError(t('m.folders.could_not_move'), e)
      return false
    } finally {
      this.moving = false
    }
    const n = ids.length
    const content = folder === null ? t('m.folders.moved_unsorted', { VAR_PLURAL: n, PH: n }) : t('m.folders.moved', { VAR_PLURAL: n, PH: n, PH_1: this.byId(folder)?.name ?? '' })
    if (groups && groups.length) {
      toastService.show({
        content,
        delay: 8000,
        level: 'success',
        persistent: false,
        actionName: t('detail.undo'),
        action: () => void this.undo(groups),
      })
    } else toastService.showSuccess(content)
    if (inFolder && here !== folder) ws.removeLocal(ids)
    else {
      ws.patchMany(ids, { folder })
      ws.selected = ws.selected.filter((i) => !ids.includes(i))
    }
    void this.refresh()
    return true
  }

  private async undo(groups: { folder: number | null; ids: number[] }[]) {
    try {
      for (const g of groups) await moveDocuments(g.ids, g.folder)
    } catch (e) {
      toastService.showError(t('m.folders.could_not_move'), e)
    }
    void this.refresh()
    void ws.reload()
  }

  /** Re-parent a folder (drag in the tree / onto a tile); the server refuses cycles and duplicate names */
  async reparent(id: number, parent: number | null): Promise<boolean> {
    if (this.moving) return false
    this.moving = true
    try {
      await moveFolder(id, parent)
    } catch (e) {
      toastService.showError(t('m.folders.could_not_move_folder'), e)
      return false
    } finally {
      this.moving = false
    }
    if (parent !== null && this.collapsed.has(parent)) this.toggle(parent)
    await this.refresh()
    return true
  }

  /** Switch the folder view on/off (stored per user in ui_settings) */
  async setEnabled(on: boolean) {
    const { setSetting, storeSettings } = await import('../../stores/settingsSave.svelte')
    setSetting(FOLDERS_KEY, on)
    await storeSettings()
    if (on) void this.refresh()
  }
}

export const folders = new FolderState()
