// data/document-suggestions.ts (mergeSuggestions) + counting logic from suggestions-dropdown.component.ts
import type { DocumentSuggestions } from '../../../api/v1'

export type SuggestionSource = 'ml' | 'ai' | 'both'

const union = <T>(a: T[] = [], b: T[] = []): T[] => [...new Set([...a, ...b])]

export function mergeSuggestions(a: DocumentSuggestions | null | undefined, b: DocumentSuggestions): DocumentSuggestions {
  if (!a) return b
  return {
    title: a.title || b.title,
    tags: union(a.tags, b.tags),
    suggested_tags: union(a.suggested_tags, b.suggested_tags),
    correspondents: union(a.correspondents, b.correspondents),
    suggested_correspondents: union(a.suggested_correspondents, b.suggested_correspondents),
    document_types: union(a.document_types, b.document_types),
    suggested_document_types: union(a.suggested_document_types, b.suggested_document_types),
    storage_paths: union(a.storage_paths, b.storage_paths),
    suggested_storage_paths: union(a.suggested_storage_paths, b.suggested_storage_paths),
    dates: union(a.dates, b.dates),
  }
}

export interface Applied {
  tags?: number[]
  correspondent?: number | null
  documentType?: number | null
  storagePath?: number | null
  title?: string | null
  created?: string | null
}

const countUnapplied = (suggested: number[] | undefined, applied: number[] | undefined) =>
  (suggested ?? []).filter((id) => !(applied ?? []).includes(id)).length

export function novelSuggestions(s: DocumentSuggestions | null | undefined): number {
  return (s?.suggested_correspondents?.length ?? 0) + (s?.suggested_tags?.length ?? 0) + (s?.suggested_document_types?.length ?? 0)
}

export function fieldSuggestions(s: DocumentSuggestions | null | undefined, a: Applied): number {
  const reusable =
    countUnapplied(s?.tags, a.tags) +
    countUnapplied(s?.correspondents, a.correspondent ? [a.correspondent] : []) +
    countUnapplied(s?.document_types, a.documentType ? [a.documentType] : []) +
    countUnapplied(s?.storage_paths, a.storagePath ? [a.storagePath] : [])
  const title = s?.title && s.title !== a.title ? 1 : 0
  const dates = (s?.dates ?? []).filter((d) => !a.created || d !== a.created).length
  return reusable + title + dates
}

export function noSuggestions(s: DocumentSuggestions | null | undefined, loading: boolean): boolean {
  return (
    !loading &&
    s != null &&
    !s.title &&
    !s.tags?.length &&
    !s.suggested_tags?.length &&
    !s.correspondents?.length &&
    !s.suggested_correspondents?.length &&
    !s.document_types?.length &&
    !s.suggested_document_types?.length &&
    !s.storage_paths?.length &&
    !s.suggested_storage_paths?.length &&
    !s.dates?.length
  )
}
