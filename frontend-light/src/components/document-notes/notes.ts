import type { DocumentNote, NoteFormat, User } from '../../api/v1'
import { renderMarkdown } from '../chat/markdown'

/** DocumentNotesComponent.displayName: "First name Last name (user)" or user name */
export function noteDisplayName(note: DocumentNote, users: Map<number, User>): string {
  if (!note.user) return ''
  const id = typeof note.user === 'number' ? note.user : note.user.id
  const user = users.get(id)
  if (!user) return ''
  const parts: string[] = []
  if (user.first_name) parts.push(user.first_name)
  if (user.last_name) parts.push(user.last_name)
  if (user.username) parts.push(parts.length > 0 ? `(${user.username})` : user.username)
  return parts.join(' ')
}

/** Effective format of a note: only an explicit "markdown" renders as Markdown; missing/unknown = plain. */
export function noteFormat(note: Pick<DocumentNote, 'format'>): NoteFormat {
  return note.format === 'markdown' ? 'markdown' : 'plain'
}

/**
 * Safe HTML for a Markdown note. Notes are written by other users, so this goes through the chat renderer:
 * all text is HTML-escaped, raw HTML and images stay text, links only http/https/mailto with rel=noopener.
 */
export function renderNoteMarkdown(text: string | null | undefined): string {
  return renderMarkdown(text)
}
