// Selection lists from data/mail-account.ts and mail-rule-edit-dialog.component.ts (values like the enums).
import { msg } from '../../../lib/i18n'

export const IMAPSecurity = { None: 1, SSL: 2, STARTTLS: 3 } as const
export const MailAccountType = { IMAP: 1, Gmail_OAuth: 2, Outlook_OAuth: 3 } as const
export const MailAction = { Delete: 1, Move: 2, MarkRead: 3, Flag: 4, Tag: 5 } as const
export const MailMetadataCorrespondentOption = { FromNothing: 1, FromEmail: 2, FromName: 3, FromCustom: 4 } as const

export const IMAP_SECURITY_OPTIONS = [
  { id: IMAPSecurity.None, name: msg('manage.mail.no_encryption') },
  { id: IMAPSecurity.SSL, name: msg('manage.mail.ssl') },
  { id: IMAPSecurity.STARTTLS, name: msg('manage.mail.starttls') },
]
export const ATTACHMENT_TYPE_OPTIONS = [
  { id: 1, name: msg('manage.mail.only_process_attachments') },
  { id: 2, name: msg("manage.mail.process_all_files_including") },
]
export const CONSUMPTION_SCOPE_OPTIONS = [
  { id: 1, name: msg('manage.mail.only_process_attachments') },
  { id: 2, name: msg('manage.mail.process_message_eml') },
  { id: 3, name: msg('manage.mail.process_message_eml_attachments') },
]
export const PDF_LAYOUT_OPTIONS = [
  { id: 0, name: msg('manage.mail.system_default') },
  { id: 1, name: msg('manage.mail.text_html') },
  { id: 2, name: msg('manage.mail.html_text') },
  { id: 3, name: msg('manage.mail.html_only') },
  { id: 4, name: msg('manage.mail.text_only') },
]
export const ACTION_OPTIONS = [
  { id: MailAction.Delete, name: msg('common.delete') },
  { id: MailAction.Move, name: msg('manage.mail.move_specified_folder') },
  { id: MailAction.MarkRead, name: msg("manage.mail.mark_read_dont_process") },
  { id: MailAction.Flag, name: msg("manage.mail.flag_mail_dont_process") },
  { id: MailAction.Tag, name: msg("manage.mail.tag_mail_specified_tag") },
]
export const METADATA_TITLE_OPTIONS = [
  { id: 1, name: msg('manage.mail.use_subject_title') },
  { id: 2, name: msg('manage.mail.use_attachment_filename_title') },
  { id: 3, name: msg('manage.mail.not_assign_title_rule') },
]
export const METADATA_CORRESPONDENT_OPTIONS = [
  { id: MailMetadataCorrespondentOption.FromNothing, name: msg('manage.mail.not_assign_correspondent') },
  { id: MailMetadataCorrespondentOption.FromEmail, name: msg('manage.mail.use_mail_address') },
  { id: MailMetadataCorrespondentOption.FromName, name: msg('manage.mail.use_name_mail_address') },
  { id: MailMetadataCorrespondentOption.FromCustom, name: msg('manage.mail.use_correspondent_selected_below') },
]

/** Translated copy of a selection list (t() at runtime) */
export const translated = <T extends { name: string }>(opts: T[], t: (s: string) => string) => opts.map((o) => ({ ...o, name: t(o.name) }))

export const showActionParamField = (action: number) => action == MailAction.Move || action == MailAction.Tag
export const showCorrespondentField = (from: number) => from == MailMetadataCorrespondentOption.FromCustom

/** copyMailRule: copy without ID, name with " (copy)" */
export function copyRule<T extends { id?: number | null; name: string }>(rule: T): T {
  return { ...rule, id: null, name: `${rule.name} (copy)` }
}
