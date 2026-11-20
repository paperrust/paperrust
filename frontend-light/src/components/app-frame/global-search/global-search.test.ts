import { describe, expect, it } from 'vitest'
import { SECTIONS, disablePrimary, disableSecondary, editDialogSize, filterDocumentsPath, flatten, fullSearchRuleType, hasSecondary } from './global-search'

describe('global-search (GlobalSearchComponent)', () => {
  it('Gruppen in Template-Reihenfolge', () => {
    expect(SECTIONS.map((s) => s.key)).toEqual([
      'documents',
      'saved_views',
      'tags',
      'correspondents',
      'document_types',
      'storage_paths',
      'users',
      'groups',
      'custom_fields',
      'mail_accounts',
      'mail_rules',
      'workflows',
    ])
  })
  it('Knöpfe und Sperren wie upstream', () => {
    expect(hasSecondary('documents')).toBe(true)
    expect(hasSecondary('tags')).toBe(true)
    expect(hasSecondary('saved_views')).toBe(false)
    expect(hasSecondary('workflows')).toBe(false)
    expect(disablePrimary('users', false)).toBe(true)
    expect(disablePrimary('tags', false)).toBe(false)
    expect(disablePrimary('mail_rules', false)).toBe(false)
    expect(disableSecondary('documents', false)).toBe(false)
    expect(disableSecondary('tags', false)).toBe(true)
    expect(editDialogSize('users')).toBe('lg')
    expect(editDialogSize('workflows')).toBe('xl')
    expect(editDialogSize('tags')).toBeUndefined()
  })
  it('Dokumente filtern: /documents mit Regel, Sortierung, Seite 1', () => {
    expect(filterDocumentsPath(6, 4, 'title', false)).toBe('documents?tags__id__all=4&sort=title&page=1')
    expect(filterDocumentsPath(26, 1, null, true)).toBe('documents?correspondent__id__in=1&sort=created&reverse=1&page=1')
  })
  it('Volltextsuche: erweitert → FILTER_FULLTEXT_QUERY, sonst FILTER_SIMPLE_TEXT', () => {
    expect(fullSearchRuleType('advanced')).toBe(20)
    expect(fullSearchRuleType('title-content')).toBe(49)
    expect(fullSearchRuleType(undefined)).toBe(49)
  })
  it('flatten zählt Einträge in Gruppenreihenfolge', () => {
    const r: any = {
      total: 2,
      documents: [{ id: 1 }],
      saved_views: [],
      tags: [{ id: 2 }],
      correspondents: [],
      document_types: [],
      storage_paths: [],
      users: [],
      groups: [],
      custom_fields: [],
      mail_accounts: [],
      mail_rules: [],
      workflows: [],
    }
    expect(flatten(r).map((x) => x.type)).toEqual(['documents', 'tags'])
    expect(flatten(null)).toEqual([])
  })
})
