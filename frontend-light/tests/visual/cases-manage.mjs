// Image comparison S7 administration (master data) and S8 settings – cases for tests/visual/compare.mjs.
// Same selectors in both UIs; open states (dialogs, selection) via clicks.
const L = (host) => `${host} tbody tr.data-row`
const NAME = (host) => `${host} td.name-cell button`
const MODAL = '.modal-dialog .modal-footer'
const CREATE = 'pngx-page-header button:has(pngx-bs-icon[name="plus-circle"])'

export default [
  { id: 'manage-tags', path: 'attributes/tags', ready: L('pngx-tag-list') },
  { id: 'manage-correspondents', path: 'attributes/correspondents', ready: L('pngx-correspondent-list') },
  { id: 'manage-documenttypes', path: 'attributes/documenttypes', ready: L('pngx-document-type-list') },
  { id: 'manage-storagepaths', path: 'attributes/storagepaths', ready: L('pngx-storage-path-list') },
  { id: 'manage-customfields', path: 'attributes/customfields', ready: 'pngx-custom-fields li.list-group-item:nth-child(2)' },
  { id: 'manage-tags-selected', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [{ click: 'pngx-tag-list tbody tr.data-row:nth-child(2) input.form-check-input' }, { click: 'pngx-tag-list tbody tr.data-row:nth-child(4) input.form-check-input' }] },
  { id: 'manage-tags-filtered', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [{ fill: ['#managementNameFilter', 'in'] }, { wait: 1500 }] },
  { id: 'manage-tag-edit', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [{ click: NAME('pngx-tag-list') }, { waitFor: MODAL }, { wait: 600 }] },
  { id: 'manage-tag-edit-perms', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [{ click: NAME('pngx-tag-list') }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button' }, { wait: 800 }] },
  { id: 'manage-tag-create', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [{ click: CREATE }, { waitFor: MODAL }, { fill: ['.modal-dialog pngx-input-color input.form-control', '#a6cee3'] }, { click: '.modal-dialog .modal-title' }, { wait: 600 }] },
  { id: 'manage-tag-delete', path: 'attributes/tags', ready: L('pngx-tag-list'), actions: [
      // Desktop: button "Delete"; mobile: action menu, then "Delete" (a click on the dialog title has no effect on the desktop)
      { click: 'pngx-tag-list tbody tr.data-row:first-child button:visible:is(.btn-outline-danger, .dropdown-toggle)' },
      { click: ':is(.modal-dialog .modal-title, .dropdown-menu.show button.text-danger)' },
      { waitFor: MODAL },
      { wait: 600 },
    ] },
  { id: 'manage-correspondent-edit', path: 'attributes/correspondents', ready: L('pngx-correspondent-list'), actions: [{ click: NAME('pngx-correspondent-list') }, { waitFor: MODAL }, { wait: 600 }] },
  { id: 'manage-documenttype-edit', path: 'attributes/documenttypes', ready: L('pngx-document-type-list'), actions: [{ click: NAME('pngx-document-type-list') }, { waitFor: MODAL }, { wait: 600 }] },
  { id: 'manage-storagepath-edit', path: 'attributes/storagepaths', ready: L('pngx-storage-path-list'), actions: [{ click: NAME('pngx-storage-path-list') }, { waitFor: MODAL }, { wait: 1500 }] },
  { id: 'manage-customfield-edit', path: 'attributes/customfields', ready: 'pngx-custom-fields li.list-group-item:nth-child(2)', actions: [{ click: 'pngx-custom-fields li.list-group-item:nth-child(2) button.btn-link' }, { waitFor: MODAL }, { wait: 600 }] },
  { id: 'settings-general', path: 'settings/general', ready: 'pngx-settings #dateFormatShort', actions: [{ wait: 1000 }] },
  { id: 'settings-documents', path: 'settings/documents', ready: 'pngx-settings form ul.nav-tabs', actions: [{ wait: 1200 }] },
  { id: 'settings-permissions', path: 'settings/permissions', ready: 'pngx-settings form ul.nav-tabs', actions: [{ wait: 1200 }] },
  { id: 'settings-notifications', path: 'settings/notifications', ready: 'pngx-settings form ul.nav-tabs', actions: [{ wait: 1200 }] },
  // Part 2: saved views
  { id: 'savedviews', path: 'savedviews', ready: 'pngx-saved-views li.list-group-item.py-3', actions: [{ wait: 800 }] },
  { id: 'savedviews-full', path: 'savedviews', ready: 'pngx-saved-views li.list-group-item.py-3', fullPage: true, actions: [{ wait: 800 }] },
  { id: 'savedviews-confirm', path: 'savedviews', ready: 'pngx-saved-views li.list-group-item.py-3', actions: [{ click: 'pngx-saved-views pngx-confirm-button button' }, { wait: 500 }] },
  { id: 'savedviews-perms', path: 'savedviews', ready: 'pngx-saved-views li.list-group-item.py-3', actions: [{ click: 'pngx-saved-views button:has(pngx-bs-icon[name="person-fill-lock"])' }, { waitFor: MODAL }, { wait: 600 }] },
  { id: 'savedviews-icon-open', path: 'savedviews', ready: 'pngx-saved-views li.list-group-item.py-3', actions: [{ click: 'pngx-saved-views li:first-child pngx-input-select .ng-select-container' }, { wait: 600 }] },
]

export const samples = ['manage-tags/en-US/light/desktop', 'manage-tag-edit/de-DE/dark/desktop', 'settings-general/de-DE/light/mobile']
