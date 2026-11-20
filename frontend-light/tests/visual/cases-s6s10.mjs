// S6 upload/notifications and S10 dashboard (P4). Included by compare.mjs (cases-*.mjs).
// The reference DB stores the dashboard views of admin at the top level (Angular reads
// saved_views.dashboard_views_visible_ids) → dashboard is empty, welcome widget visible. For the widget cases
// VIEWS sets views 1 and 2 (+ sidebar 1,2,4) via /api/ui_settings/ and reloads; NONE restores
// the empty state. Changes the test DB → afterwards tests/server.sh reset.
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
// existing original of the reference data (tests/server.sh reset unpacks it) → no consumer runs in the test server → final state "Upload complete, waiting..." in both UIs
const SAMPLE = path.join(ROOT, '.e2e/data/media/documents/originals/0000001.pdf')
const DASH = 'pngx-statistics-widget .badge.bg-primary'
const views = (dash, side) =>
  `(async () => { const csrf = (document.cookie.match(/csrftoken=([^;]+)/) || [])[1]; const s = (await (await fetch('/api/ui_settings/')).json()).settings; s.saved_views = { ...(s.saved_views || {}), dashboard_views_visible_ids: ${JSON.stringify(dash)}, sidebar_views_visible_ids: ${JSON.stringify(side)} }; await fetch('/api/ui_settings/', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-CSRFToken': csrf }, body: JSON.stringify({ settings: s }) }) })()`
const VIEWS = [{ eval: views([1, 2], [1, 2, 4]), wait: 100 }, { reload: true, waitFor: 'pngx-saved-view-widget table tbody tr', wait: 1500 }]
const NONE = [{ eval: views([], []), wait: 100 }, { reload: true, waitFor: DASH, wait: 1000 }]
// drag file over the page (document:dragover like FileDropComponent)
const DRAG = `(() => { const dt = new DataTransfer(); dt.items.add(new File(['x'], 'x.pdf', { type: 'application/pdf' })); document.dispatchEvent(new DragEvent('dragover', { dataTransfer: dt, bubbles: true, cancelable: true })) })()`

export default [
  { id: 'dashboard-welcome', path: 'dashboard', ready: DASH, actions: [...NONE] },
  { id: 'dashboard-views', path: 'dashboard', ready: DASH, actions: [...VIEWS] },
  { id: 'dashboard-full', path: 'dashboard', ready: DASH, fullPage: true, actions: [...VIEWS] },
  { id: 'dashboard-dragover', path: 'dashboard', ready: DASH, actions: [...NONE, { eval: DRAG, wait: 800 }] },
  { id: 'dashboard-notifications', path: 'dashboard', ready: DASH, actions: [...NONE, { click: '#notificationsDropdown' }] },
  { id: 'dashboard-upload', path: 'dashboard', ready: DASH, actions: [...NONE, { upload: ['#navbar-file-upload', SAMPLE], waitFor: 'pngx-upload-status-tray ngb-progressbar', wait: 4000 }] },
]

export const samples = ['dashboard-views/de-DE/light/desktop', 'dashboard-welcome/en-US/dark/mobile', 'dashboard-upload/en-US/light/desktop']
