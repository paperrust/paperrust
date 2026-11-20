// P4 S5b document view – open states (calendar popup, create dialog, reprocess, share links)
// and calendar popup in the date filter of the list. Included by compare.mjs (cases-*.mjs). Changes no data.
// No case for the WebSocket toast: pl-server does not send document_updated via the WebSocket on PATCH.
// No case for the email dialog: email is not set up in the test server (menu entry missing in both).
const D = 'pngx-document-detail .thumb-preview.fade'
const T = 'table tbody tr'
const W = { wait: 1500 }

export default [
  { id: 'detail-datepicker-open', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [W, { click: 'pngx-input-date button.calendar', wait: 800 }] },
  { id: 'detail-create-correspondent', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [W, { click: 'pngx-input-select button:has(pngx-bs-icon[name=plus])', waitFor: '.modal-dialog .modal-footer', wait: 800 }] },
  { id: 'detail-reprocess-confirm', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [W, { click: '#actionsDropdown' }, { click: '[aria-labelledby=actionsDropdown] button', waitFor: '.modal-dialog .modal-footer', wait: 800 }] },
  { id: 'detail-share-links', path: 'documents/191/details', ready: D, readyState: 'attached', actions: [W, { click: '#sendDropdown' }, { click: '[aria-labelledby=sendDropdown] button', waitFor: '.modal-dialog .modal-footer', wait: 1500 }] },
  { id: 'doc-dates-picker', path: 'documents', mode: 'table', ready: T, actions: [{ click: 'pngx-filter-editor pngx-dates-dropdown .dropdown-toggle' }, { click: 'pngx-dates-dropdown .dropdown-menu .input-group button', wait: 800 }] },
]

export const samples = ['detail-datepicker-open/de-DE/light/desktop', 'detail-share-links/en-US/dark/desktop', 'detail-create-correspondent/en-US/light/mobile', 'doc-dates-picker/de-DE/light/desktop']
