import type { DocumentNote, User } from '../../api/v1'

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
