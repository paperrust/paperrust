// Texts for roles and topics (fixed roles are translated, custom roles show their name).
import { t } from '../../lib/i18n'
import type { Access, Level, Role, Summary } from './roles'

const ROLE_NAME: Record<string, string> = {
  read: 'm.admin.role_read',
  edit: 'm.admin.role_edit',
  manage: 'm.admin.role_manage',
  admin: 'm.admin.role_admin',
}
const ROLE_DESC: Record<string, string> = {
  read: 'm.admin.role_read_desc',
  edit: 'm.admin.role_edit_desc',
  manage: 'm.admin.role_manage_desc',
  admin: 'm.admin.role_admin_desc',
}
export const roleName = (r: Role) => (r.key && r.builtin ? t(ROLE_NAME[r.key]) : r.name || (r.key ? t(ROLE_NAME[r.key]) : ''))
export const roleDesc = (r: Role) => (r.key && r.builtin ? t(ROLE_DESC[r.key]) : r.description)

const TOPIC: Record<string, [string, string]> = {
  documents: ['m.admin.topic_documents', 'm.admin.topic_documents_hint'],
  organize: ['m.admin.topic_organize', 'm.admin.topic_organize_hint'],
  views: ['m.admin.topic_views', 'm.admin.topic_views_hint'],
  automation: ['m.admin.topic_automation', 'm.admin.topic_automation_hint'],
  system: ['m.admin.topic_system', 'm.admin.topic_system_hint'],
}
export const topicName = (k: string) => (TOPIC[k] ? t(TOPIC[k][0]) : k)
export const topicHint = (k: string) => (TOPIC[k] ? t(TOPIC[k][1]) : '')

const MODEL: Record<string, string> = {
  document: 'common.documents_2',
  note: 'common.notes',
  sharelink: 'common.share_links',
  sharelinkbundle: 'm.admin.model_sharelinkbundle',
  logentry: 'common.history',
  tag: 'common.tags',
  correspondent: 'common.correspondents',
  documenttype: 'common.document_types',
  storagepath: 'common.storage_paths',
  customfield: 'common.custom_fields',
  savedview: 'common.saved_views',
  workflow: 'common.workflows',
  mailaccount: 'manage.mail.mail_accounts',
  mailrule: 'manage.mail.mail_rules',
  processedmail: 'm.admin.model_processedmail',
  paperlesstask: 'common.tasks',
  user: 'common.users',
  group: 'common.groups',
  applicationconfiguration: 'common.configuration',
}
/** Name of a model (`app.model`) for the fine-tuning per model */
export function modelName(m: string): string {
  const k = m.slice(m.indexOf('.') + 1)
  return MODEL[k] ? t(MODEL[k]) : k
}

/** Label of the role in the list */
export function accessLabel(a: Access): string {
  if (a.kind === 'role') return roleName(a.role)
  if (a.kind === 'individual') return t('m.admin.role_individual')
  return t('m.admin.role_none')
}

const LEVEL: Record<Level, string> = {
  none: '',
  view: 'm.admin.lvl_view',
  edit: 'm.admin.lvl_edit',
  full: 'm.admin.lvl_full',
  partial: 'm.admin.partial',
}
/** effective permissions as one sentence, e.g. "Documents and notes: view and edit · Organization: view · No access: …" */
export function summarySentence(s: Summary): string {
  if (s.all) return t('m.admin.role_admin_desc')
  const yes = s.items.filter((i) => i.level !== 'none').map((i) => `${topicName(i.topic)}: ${t(LEVEL[i.level])}`)
  const no = s.items.filter((i) => i.level === 'none').map((i) => topicName(i.topic))
  if (!yes.length) return t('m.admin.eff_nothing')
  return [...yes, ...(no.length ? [t('m.admin.eff_no_access', { PH: no.join(', ') })] : [])].join(' · ')
}
