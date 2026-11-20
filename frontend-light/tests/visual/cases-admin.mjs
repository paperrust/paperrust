// Image comparison administration (second P4 agent, part 2): trash, logs, tasks, configuration.
const MODAL = '.modal-dialog .modal-footer'
const TR = 'pngx-trash tbody tr.data-row'
const TK = 'pngx-tasks table.tasks-table tbody tr'
const CF = 'pngx-config .card .card-title'
const nav = (i) => `pngx-config ul.nav-tabs li:nth-child(${i}) a`

export default [
  { id: 'admin-trash', path: 'trash', ready: TR, actions: [{ wait: 600 }] },
  { id: 'admin-trash-selected', path: 'trash', ready: TR, actions: [{ click: `${TR}:nth-child(2) input.form-check-input` }, { click: `${TR}:nth-child(3) input.form-check-input` }] },
  { id: 'admin-trash-empty', path: 'trash', ready: TR, actions: [{ click: 'pngx-page-header button.btn-outline-danger >> nth=1' }, { waitFor: MODAL }, { wait: 500 }] },
  { id: 'admin-logs', path: 'logs', ready: 'pngx-logs .log-container', actions: [{ click: '#autoRefreshSwitch' }, { wait: 800 }] },
  { id: 'admin-logs-mail', path: 'logs', ready: 'pngx-logs .log-container', actions: [{ click: '#autoRefreshSwitch' }, { click: 'pngx-logs ul.nav-tabs li:nth-child(2) a' }, { wait: 1000 }] },
  { id: 'admin-tasks', path: 'tasks', ready: TK, actions: [{ click: '#autoRefreshSwitch' }, { wait: 800 }] },
  { id: 'admin-tasks-full', path: 'tasks', ready: TK, fullPage: true, actions: [{ click: '#autoRefreshSwitch' }, { wait: 800 }] },
  // Expand details: on the desktop only for long result texts (test data: none) → mobile only
  { id: 'admin-tasks-expanded', path: 'tasks', ready: TK, viewports: ['mobile'], actions: [{ click: '#autoRefreshSwitch' }, { click: 'pngx-tasks td.d-lg-none button.btn-link >> nth=1' }, { wait: 800 }] },
  { id: 'admin-tasks-type', path: 'tasks', ready: TK, actions: [{ click: '#autoRefreshSwitch' }, { click: 'pngx-tasks .task-controls .dropdown-toggle >> nth=0' }, { wait: 500 }] },
  { id: 'admin-tasks-attention', path: 'tasks', ready: TK, actions: [{ click: '#autoRefreshSwitch' }, { click: 'label[for=section-needs_attention]' }, { wait: 1200 }] },
  { id: 'admin-config', path: 'config', ready: CF, actions: [{ wait: 600 }] },
  { id: 'admin-config-ocr', path: 'config', ready: CF, actions: [{ click: nav(2) }, { wait: 600 }] },
  { id: 'admin-config-ocr-full', path: 'config', ready: CF, fullPage: true, actions: [{ click: nav(2) }, { wait: 600 }] },
  { id: 'admin-config-barcode', path: 'config', ready: CF, actions: [{ click: nav(3) }, { wait: 600 }] },
  { id: 'admin-config-ai', path: 'config', ready: CF, actions: [{ click: nav(4) }, { wait: 600 }] },
  { id: 'admin-config-dirty', path: 'config', ready: CF, actions: [{ fill: ['pngx-config pngx-input-text input', 'PaperRust Test'] }, { click: 'pngx-config h6 >> nth=0' }, { wait: 500 }] },
  // Users & groups
  { id: 'admin-users', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', actions: [{ wait: 600 }] },
  { id: 'admin-user-edit', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', actions: [{ click: 'pngx-users-groups li.list-group-item button.btn-link >> nth=1' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'admin-user-edit-full', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', fullPage: true, actions: [{ click: 'pngx-users-groups li.list-group-item button.btn-link >> nth=1' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'admin-user-create', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', actions: [{ click: 'pngx-users-groups h4 button >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'admin-group-edit', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', actions: [{ click: 'pngx-users-groups ul.list-group >> nth=1 >> button.btn-link >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'admin-user-delete', path: 'usersgroups', ready: 'pngx-users-groups li.list-group-item button.btn-link', actions: [{ click: 'pngx-users-groups .btn-outline-danger >> nth=2' }, { waitFor: MODAL }, { wait: 500 }] },
  // E-Mail
  { id: 'mail', path: 'mail', ready: 'pngx-mail .row.fade.show', actions: [{ wait: 600 }] },
  { id: 'mail-account-edit', path: 'mail', ready: 'pngx-mail .row.fade.show', actions: [{ click: 'pngx-mail ul.list-group >> nth=0 >> button.btn-link >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'mail-account-create', path: 'mail', ready: 'pngx-mail .row.fade.show', actions: [{ click: 'pngx-mail h4 >> nth=0 >> button' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'mail-rule-edit', path: 'mail', ready: 'pngx-mail .row.fade.show', actions: [{ click: 'pngx-mail ul.list-group >> nth=1 >> button.btn-link >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'mail-rule-edit-full', path: 'mail', ready: 'pngx-mail .row.fade.show', fullPage: true, actions: [{ click: 'pngx-mail ul.list-group >> nth=1 >> button.btn-link >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'mail-rule-create', path: 'mail', ready: 'pngx-mail .row.fade.show', actions: [{ click: 'pngx-mail h4 >> nth=1 >> button' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'mail-processed', path: 'mail', ready: 'pngx-mail .row.fade.show', viewports: ['desktop'], actions: [{ click: 'pngx-mail button:has(pngx-bs-icon[name="clock-history"]) >> nth=0' }, { waitFor: '.modal-dialog .modal-body' }, { wait: 1200 }] },
  { id: 'mail-perms', path: 'mail', ready: 'pngx-mail .row.fade.show', viewports: ['desktop'], actions: [{ click: 'pngx-mail button:has(pngx-bs-icon[name="person-lock"]) >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  // Workflows (dialog: expand accordions Triggers/Actions one after the other)
  { id: 'workflows', path: 'workflows', ready: 'pngx-workflows .row.fade.show', actions: [{ wait: 600 }] },
  { id: 'workflow-edit', path: 'workflows', ready: 'pngx-workflows .row.fade.show', actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { wait: 800 }] },
  { id: 'workflow-edit-triggers', path: 'workflows', ready: 'pngx-workflows .row.fade.show', actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button >> nth=0' }, { wait: 600 }] },
  { id: 'workflow-edit-trigger1', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button >> nth=0' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1000 }] },
  { id: 'workflow-edit-trigger2', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button >> nth=0' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=1' }, { wait: 1000 }] },
  { id: 'workflow-edit-actions', path: 'workflows', ready: 'pngx-workflows .row.fade.show', actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .btn-lg.accordion-button' }, { wait: 600 }] },
  { id: 'workflow-edit-action1', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .btn-lg.accordion-button' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1200 }] },
  { id: 'workflow-edit-action2', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=0' }, { waitFor: MODAL }, { click: '.modal-dialog .btn-lg.accordion-button' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=1' }, { wait: 1200 }] },
  { id: 'workflow-edit-email', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=1' }, { waitFor: MODAL }, { click: '.modal-dialog .btn-lg.accordion-button' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1200 }] },
  { id: 'workflow-edit-trigger-filters', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=1' }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button >> nth=0' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1200 }] },
  { id: 'workflow-edit-trigger-scheduled', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=2' }, { waitFor: MODAL }, { click: '.modal-dialog .accordion-button >> nth=0' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1200 }] },
  { id: 'workflow-edit-wf3-action', path: 'workflows', ready: 'pngx-workflows .row.fade.show', fullPage: true, actions: [{ click: 'pngx-workflows li .col > button.btn-link >> nth=2' }, { waitFor: MODAL }, { click: '.modal-dialog .btn-lg.accordion-button' }, { click: '.modal-dialog .accordion .accordion .accordion-button >> nth=0' }, { wait: 1200 }] },
  { id: 'workflow-create', path: 'workflows', ready: 'pngx-workflows .row.fade.show', actions: [{ click: 'pngx-page-header button:has(pngx-bs-icon[name="plus-circle"])' }, { waitFor: MODAL }, { wait: 800 }] },
]

export const samples = ['admin-tasks/en-US/light/desktop', 'admin-config-ocr/de-DE/dark/desktop', 'admin-trash/de-DE/light/mobile']
