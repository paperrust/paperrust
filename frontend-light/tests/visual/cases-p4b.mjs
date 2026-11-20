// P4 agent B: drag dashboard widgets (cdkDrag), tour steps (ngx-ui-tour) and messages after real processing.
// The case dashboard-consumed needs a test server WITH processing (tests/server.sh with F0_ARGS="serve --worker");
// each capture uploads a new PDF with the same name but different content (otherwise duplicate). Afterwards tests/server.sh reset.
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const DASH = 'pngx-statistics-widget .badge.bg-primary'
const views = (dash) =>
  `(async () => { const csrf = (document.cookie.match(/csrftoken=([^;]+)/) || [])[1]; const s = (await (await fetch('/api/ui_settings/')).json()).settings; s.saved_views = { ...(s.saved_views || {}), dashboard_views_visible_ids: ${JSON.stringify(dash)}, dashboard_views_sort_order: [] }; await fetch('/api/ui_settings/', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-CSRFToken': csrf }, body: JSON.stringify({ settings: s }) }) })()`
const VIEWS = [{ eval: views([1, 2, 4]), wait: 100 }, { reload: true, waitFor: 'pngx-saved-view-widget table tbody tr', wait: 1500 }]
const NONE = [{ eval: views([]), wait: 100 }, { reload: true, waitFor: DASH, wait: 1000 }]
const GRIP = 'pngx-saved-view-widget .card-header pngx-bs-icon[name="grip-vertical"]'
const TOUR = 'pngx-welcome-widget button.btn-primary'

/** new PDF with text layer, always the same file name (display), random content (no duplicate) */
function freshPdf() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'pngx-visual-'))
  // > 50 characters (VALID_TEXT_LENGTH), otherwise the text layer counts as empty and OCR (Tesseract) is needed
  const text = `PaperRust visual comparison sample document ${Date.now()} ${Math.random().toString(36).slice(2)}`
  const stream = `BT /F1 24 Tf 72 760 Td (${text}) Tj ET`
  const objs = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    '<< /Type /Pages /Kids [3 0 R] /Count 1 >>',
    '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>',
    `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`,
    '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
  ]
  let out = '%PDF-1.4\n'
  const offs = objs.map((o, i) => {
    const at = out.length
    out += `${i + 1} 0 obj\n${o}\nendobj\n`
    return at
  })
  const xref = out.length
  out += `xref\n0 6\n0000000000 65535 f \n${offs.map((o) => String(o).padStart(10, '0') + ' 00000 n \n').join('')}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
  const file = path.join(dir, 'scan-2026-11-09.pdf')
  fs.writeFileSync(file, out, 'latin1')
  return file
}

export default [
  // Widget grabbed by the handle and dragged 260 px down (preview, placeholder, shifted siblings)
  { id: 'dashboard-drag', path: 'dashboard', ready: DASH, keepMouse: true, actions: [...VIEWS, { dragHold: [GRIP, 0, 260], wait: 800 }] },
  // Tour: step 1 (dashboard header) and step 3 (document list, other screen)
  { id: 'dashboard-tour', path: 'dashboard', ready: DASH, actions: [...NONE, { click: TOUR, wait: 2500 }] },
  { id: 'dashboard-tour-docs', path: 'dashboard', ready: DASH, actions: [...NONE, { click: TOUR, wait: 2500 }, { click: '.tour-step button:has-text("»")', wait: 1500 }, { click: '.tour-step button:has-text("»")', waitFor: 'pngx-document-list table tbody tr, pngx-document-card-small', wait: 3000 }] },
  // real processing: upload list with "Open document", then bell with "Document … was added"
  { id: 'dashboard-consumed', path: 'dashboard', ready: DASH, actions: [...NONE, { upload: ['#navbar-file-upload', freshPdf], waitFor: 'pngx-upload-status-tray .btn-open', wait: 1500 }] },
  { id: 'dashboard-consumed-bell', path: 'dashboard', ready: DASH, actions: [...NONE, { upload: ['#navbar-file-upload', freshPdf], waitFor: 'pngx-upload-status-tray .btn-open', wait: 1000 }, { click: '#notificationsDropdown', wait: 800 }] },
]

export const samples = ['dashboard-drag/en-US/light/desktop', 'dashboard-tour/de-DE/dark/desktop', 'dashboard-consumed/en-US/light/desktop']
