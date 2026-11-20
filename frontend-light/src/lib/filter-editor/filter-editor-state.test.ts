// Ported from filter-editor.component.spec.ts (all assertions about produced rules / states).
// DOM interactions are replaced by the calls the template makes.
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'
import {
  CustomFieldDataType,
  CustomFieldQueryLogicalOperator,
  CustomFieldQueryOperator,
} from '../custom-field-query'
import {
  FILTER_ADDED_AFTER,
  FILTER_ADDED_BEFORE,
  FILTER_ADDED_FROM,
  FILTER_ADDED_TO,
  FILTER_ASN,
  FILTER_ASN_GT,
  FILTER_ASN_ISNULL,
  FILTER_ASN_LT,
  FILTER_CORRESPONDENT,
  FILTER_CREATED_AFTER,
  FILTER_CREATED_BEFORE,
  FILTER_CREATED_FROM,
  FILTER_CREATED_TO,
  FILTER_CUSTOM_FIELDS_QUERY,
  FILTER_CUSTOM_FIELDS_TEXT,
  FILTER_DOCUMENT_TYPE,
  FILTER_DOES_NOT_HAVE_CORRESPONDENT,
  FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE,
  FILTER_DOES_NOT_HAVE_STORAGE_PATH,
  FILTER_DOES_NOT_HAVE_TAG,
  FILTER_FULLTEXT_MORELIKE,
  FILTER_FULLTEXT_QUERY,
  FILTER_HAS_ANY_TAG,
  FILTER_HAS_CORRESPONDENT_ANY,
  FILTER_HAS_CUSTOM_FIELDS_ALL,
  FILTER_HAS_CUSTOM_FIELDS_ANY,
  FILTER_HAS_DOCUMENT_TYPE_ANY,
  FILTER_HAS_DUPLICATES,
  FILTER_HAS_STORAGE_PATH_ANY,
  FILTER_HAS_TAGS_ALL,
  FILTER_HAS_TAGS_ANY,
  FILTER_MIME_TYPE,
  FILTER_OWNER,
  FILTER_OWNER_ANY,
  FILTER_OWNER_DOES_NOT_INCLUDE,
  FILTER_OWNER_ISNULL,
  FILTER_SHARED_BY_USER,
  FILTER_SIMPLE_TEXT,
  FILTER_SIMPLE_TITLE,
  FILTER_STORAGE_PATH,
  FILTER_TITLE,
  FILTER_TITLE_CONTENT,
  NEGATIVE_NULL_FILTER_VALUE,
} from '../filter-rule-type'
import type { FilterRule } from '../filter-rules'
import { type CustomFieldQueryAtom, type CustomFieldQueryExpression } from './custom-field-queries'
import { RelativeDate } from './dates'
import { FilterEditorState, type MoreLikeDocument } from './filter-editor-state'
import { OwnerFilterType } from './permissions-model'
import { Intersection, LogicalOperator } from './selection-model'

const tags = [
  { id: 2, name: 'Tag2' },
  { id: 3, name: 'Tag3' },
]
const correspondents = [
  { id: 12, name: 'Corresp12' },
  { id: 13, name: 'Corresp13' },
]
const document_types = [
  { id: 22, name: 'DocType22' },
  { id: 23, name: 'DocType23' },
]
const storage_paths = [
  { id: 32, name: 'StoragePath32' },
  { id: 33, name: 'StoragePath33' },
]
const custom_fields = [
  { id: 42, data_type: CustomFieldDataType.String, name: 'CustomField42' },
  { id: 43, data_type: CustomFieldDataType.String, name: 'CustomField43' },
]
const users = [{ id: 1, username: 'user1' }]

describe('FilterEditorState', () => {
  let component: FilterEditorState
  let getDocument: Mock<(id: number) => Promise<MoreLikeDocument>>
  let autocomplete: Mock<(term: string) => Promise<string[]>>
  let setSearchQuery: Mock<(query: string) => void>

  const tick = (ms = 0) => vi.advanceTimersByTime(ms)
  const strip = (items: any[]) => items.map((i) => ({ id: i.id, name: i.name }))

  beforeEach(() => {
    getDocument = vi.fn<(id: number) => Promise<MoreLikeDocument>>()
    autocomplete = vi.fn<(term: string) => Promise<string[]>>(async () => [])
    setSearchQuery = vi.fn<(query: string) => void>()
    component = new FilterEditorState({
      currentUserId: users[0].id,
      users,
      tags,
      correspondents,
      documentTypes: document_types,
      storagePaths: storage_paths,
      customFields: custom_fields,
      getDocument,
      autocomplete,
      setSearchQuery,
    })
    component.filterRules = []
    vi.useFakeTimers()
  })

  afterEach(() => {
    component.destroy()
    vi.useRealTimers()
  })

  // SET filterRules

  it('should ingest text filter rules for doc title', () => {
    expect(component.textFilter).toEqual(null)
    component.filterRules = [{ rule_type: FILTER_TITLE, value: 'foo' }]
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('title')
  })

  it('should ingest text filter rules for doc title + content', () => {
    expect(component.textFilter).toEqual(null)
    component.filterRules = [{ rule_type: FILTER_SIMPLE_TEXT, value: 'foo' }]
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('title-content')
  })

  it('should ingest legacy text filter rules for doc title + content', () => {
    expect(component.textFilter).toBeNull()
    component.filterRules = [{ rule_type: FILTER_TITLE_CONTENT, value: 'legacy foo' }]
    expect(component.textFilter).toEqual('legacy foo')
    expect(component.textFilterTarget).toEqual('title-content')
  })

  it('should ingest text filter rules for doc asn', () => {
    component.filterRules = [{ rule_type: FILTER_ASN, value: 'foo' }]
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('asn')
  })

  it('should ingest text filter rules for custom fields', () => {
    component.filterRules = [{ rule_type: FILTER_CUSTOM_FIELDS_TEXT, value: 'foo' }]
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('custom-fields')
  })

  it('should ingest text filter rules for doc asn is null', () => {
    expect(component.textFilterTarget).toEqual('title-content')
    expect(component.textFilterModifier).toEqual('equals')
    component.filterRules = [{ rule_type: FILTER_ASN_ISNULL, value: 'true' }]
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('is null')
  })

  it('should ingest text filter rules for doc asn is not null', () => {
    component.filterRules = [{ rule_type: FILTER_ASN_ISNULL, value: 'false' }]
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('not null')
  })

  it('should ingest text filter rules for doc asn greater than', () => {
    component.filterRules = [{ rule_type: FILTER_ASN_GT, value: '0' }]
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('greater')
  })

  it('should ingest text filter rules for doc asn less than', () => {
    component.filterRules = [{ rule_type: FILTER_ASN_LT, value: '1000000' }]
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('less')
  })

  it('should ingest text filter rules for mime type', () => {
    component.filterRules = [{ rule_type: FILTER_MIME_TYPE, value: 'pdf' }]
    expect(component.textFilter).toEqual('pdf')
    expect(component.textFilterTarget).toEqual('mime-type')
  })

  it('should ingest filter rules for documents with duplicates', () => {
    component.filterRules = [{ rule_type: FILTER_HAS_DUPLICATES, value: 'true' }]
    expect(component.textFilterTarget).toEqual('duplicates')
    expect(component.textFilterModifier).toEqual('has-duplicates')
    expect(component.textFilterInputDisabled).toBeTruthy()
  })

  it('should ingest filter rules for documents without duplicates', () => {
    component.filterRules = [{ rule_type: FILTER_HAS_DUPLICATES, value: 'false' }]
    expect(component.textFilterTarget).toEqual('duplicates')
    expect(component.textFilterModifier).toEqual('does-not-have-duplicates')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_HAS_DUPLICATES, value: 'false' }])
  })

  it('should ingest text filter rules for fulltext query', () => {
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo,bar' }]
    expect(component.textFilter).toEqual('foo,bar')
    expect(component.textFilterTarget).toEqual('fulltext-query')
  })

  it('should ingest text filter rules for fulltext query that include date created', () => {
    expect(component.dateCreatedRelativeDate).toBeNull()
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' }]
    expect(component.dateCreatedRelativeDate).toEqual(1)
    expect(component.textFilter).toBeNull()
  })

  it('should ingest title filter rules alongside a fulltext query with date created', () => {
    component.filterRules = [
      { rule_type: FILTER_SIMPLE_TITLE, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' },
    ]
    expect(component.dateCreatedRelativeDate).toEqual(1)
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('title')
  })

  it('should ingest text filter rules for fulltext query that include date added', () => {
    expect(component.dateAddedRelativeDate).toBeNull()
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'added:[-1 week to now]' }]
    expect(component.dateAddedRelativeDate).toEqual(1)
    expect(component.textFilter).toBeNull()
  })

  it('should ingest text filter content with relative dates that are not in quick list', () => {
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'added:[-2 week to now]' }]
    expect(component.dateAddedRelativeDate).toBeNull()
    expect(component.textFilter).toEqual('added:[-2 week to now]')
    expect(component.dateCreatedRelativeDate).toBeNull()
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-2 week to now]' }]
    expect(component.dateCreatedRelativeDate).toBeNull()
    expect(component.textFilter).toEqual('created:[-2 week to now]')
  })

  it('should ingest quoted relative dates', () => {
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo,added:"yesterday"' }]
    expect(component.dateAddedRelativeDate).toEqual(RelativeDate.YESTERDAY)
    expect(component.textFilter).toEqual('foo')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo,added:"yesterday"' }])
  })

  it('should ingest text filter rules for more like', async () => {
    getDocument.mockResolvedValue({ id: 1, title: 'Foo Bar' })
    expect(component.textFilter).toEqual(null)
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_MORELIKE, value: '1' }]
    expect(component.textFilterTarget).toEqual('fulltext-morelike')
    expect(getDocument).toHaveBeenCalledWith(1)
    await vi.runAllTimersAsync()
    expect(component.textFilter).toEqual('Foo Bar')
    expect(component._moreLikeDoc).toEqual({ id: 1, title: 'Foo Bar' })
    expect(component.textFilterInputReadonly).toBeTruthy()
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_MORELIKE, value: '1' }])
  })

  it('should ingest filter rules for date created after and adjust date by 1 day', () => {
    expect(component.dateCreatedFrom).toBeNull()
    component.filterRules = [{ rule_type: FILTER_CREATED_AFTER, value: '2023-05-14' }]
    expect(component.dateCreatedFrom).toEqual('2023-05-15')
  })

  it('should ingest filter rules for date created from', () => {
    component.filterRules = [{ rule_type: FILTER_CREATED_FROM, value: '2023-05-14' }]
    expect(component.dateCreatedFrom).toEqual('2023-05-14')
  })

  it('should ingest filter rules for date created before and adjust date by 1 day', () => {
    expect(component.dateCreatedTo).toBeNull()
    component.filterRules = [{ rule_type: FILTER_CREATED_BEFORE, value: '2023-05-14' }]
    expect(component.dateCreatedTo).toEqual('2023-05-13')
  })

  it('should ingest filter rules for date created to', () => {
    component.filterRules = [{ rule_type: FILTER_CREATED_TO, value: '2023-05-14' }]
    expect(component.dateCreatedTo).toEqual('2023-05-14')
  })

  it('should ingest filter rules for date added after and adjust date by 1 day', () => {
    expect(component.dateAddedFrom).toBeNull()
    component.filterRules = [{ rule_type: FILTER_ADDED_AFTER, value: '2023-05-14' }]
    expect(component.dateAddedFrom).toEqual('2023-05-15')
  })

  it('should ingest filter rules for date added from', () => {
    component.filterRules = [{ rule_type: FILTER_ADDED_FROM, value: '2023-05-14' }]
    expect(component.dateAddedFrom).toEqual('2023-05-14')
  })

  it('should ingest filter rules for date added before and adjust date by 1 day', () => {
    expect(component.dateAddedTo).toBeNull()
    component.filterRules = [{ rule_type: FILTER_ADDED_BEFORE, value: '2023-05-14' }]
    expect(component.dateAddedTo).toEqual('2023-05-13')
  })

  it('should ingest filter rules for date added to', () => {
    component.filterRules = [{ rule_type: FILTER_ADDED_TO, value: '2023-05-14' }]
    expect(component.dateAddedTo).toEqual('2023-05-14')
    expect(component.dates.addedDateTo).toEqual('2023-05-14') // single source of truth
  })

  it('should ingest filter rules for has all tags', () => {
    expect(component.tagSelectionModel.getSelectedItems()).toHaveLength(0)
    component.filterRules = [
      { rule_type: FILTER_HAS_TAGS_ALL, value: '2' },
      { rule_type: FILTER_HAS_TAGS_ALL, value: '3' },
    ]
    expect(component.tagSelectionModel.logicalOperator).toEqual(LogicalOperator.And)
    expect(component.tagSelectionModel.getSelectedItems()).toMatchObject(tags)
    // coverage
    component.filterRules = [{ rule_type: FILTER_HAS_TAGS_ALL, value: null }]
    component.toggleTag(2)
  })

  it('should reflect ingested tag filter rules in the dropdown toggle', () => {
    expect(component.tagDropdown.isActive).toBeFalsy()
    expect(component.tagDropdown.showBadge).toBeFalsy()
    component.filterRules = [{ rule_type: FILTER_HAS_TAGS_ALL, value: '2' }]
    expect(component.tagDropdown.isActive).toBeTruthy()
    expect(component.tagDropdown.showBadge).toBeTruthy()
    component.filterRules = [{ rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: '12' }]
    expect(component.tagDropdown.isActive).toBeFalsy()
    expect(component.tagDropdown.showBadge).toBeFalsy()
  })

  it('should ingest filter rules for has any tags', () => {
    component.filterRules = [
      { rule_type: FILTER_HAS_TAGS_ANY, value: '2' },
      { rule_type: FILTER_HAS_TAGS_ANY, value: '3' },
    ]
    expect(component.tagSelectionModel.logicalOperator).toEqual(LogicalOperator.Or)
    expect(component.tagSelectionModel.getSelectedItems()).toMatchObject(tags)
    // coverage
    component.filterRules = [{ rule_type: FILTER_HAS_TAGS_ANY, value: null }]
  })

  it('should ingest filter rules for has any tag', () => {
    component.filterRules = [{ rule_type: FILTER_HAS_ANY_TAG, value: '1' }]
    expect(component.tagSelectionModel.getSelectedItems()).toHaveLength(1)
    expect(component.tagSelectionModel.get(null)).toBeTruthy()
  })

  it('should ingest filter rules for exclude tag(s)', () => {
    component.filterRules = [
      { rule_type: FILTER_DOES_NOT_HAVE_TAG, value: '2' },
      { rule_type: FILTER_DOES_NOT_HAVE_TAG, value: '3' },
    ]
    expect(component.tagSelectionModel.logicalOperator).toEqual(LogicalOperator.And)
    expect(component.tagSelectionModel.getExcludedItems()).toMatchObject(tags)
    // coverage
    component.filterRules = [{ rule_type: FILTER_DOES_NOT_HAVE_TAG, value: null }]
  })

  it('should ingest filter rules for has correspondent', () => {
    component.filterRules = [{ rule_type: FILTER_CORRESPONDENT, value: '12' }]
    expect(component.correspondentSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.correspondentSelectionModel.getSelectedItems()).toEqual([correspondents[0]])
    component.toggleCorrespondent(12) // coverage
    component.filterRules = [{ rule_type: FILTER_CORRESPONDENT, value: NEGATIVE_NULL_FILTER_VALUE.toString() }]
    expect(component.correspondentSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.correspondentSelectionModel.getExcludedItems()).toEqual([
      { id: NEGATIVE_NULL_FILTER_VALUE, name: 'Not set' },
    ])
  })

  it('should ingest filter rules for has any of correspondents', () => {
    component.filterRules = [
      { rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: '12' },
      { rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: '13' },
    ]
    expect(component.correspondentSelectionModel.logicalOperator).toEqual(LogicalOperator.Or)
    expect(component.correspondentSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.correspondentSelectionModel.getSelectedItems()).toEqual(correspondents)
    component.filterRules = [{ rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: null }]
  })

  it('should ingest filter rules for does not have any of correspondents', () => {
    component.filterRules = [
      { rule_type: FILTER_DOES_NOT_HAVE_CORRESPONDENT, value: '12' },
      { rule_type: FILTER_DOES_NOT_HAVE_CORRESPONDENT, value: '13' },
    ]
    expect(component.correspondentSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.correspondentSelectionModel.getExcludedItems()).toEqual(correspondents)
    component.filterRules = [{ rule_type: FILTER_DOES_NOT_HAVE_CORRESPONDENT, value: null }]
  })

  it('should ingest filter rules for has document type', () => {
    component.filterRules = [{ rule_type: FILTER_DOCUMENT_TYPE, value: '22' }]
    expect(component.documentTypeSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.documentTypeSelectionModel.getSelectedItems()).toEqual([document_types[0]])
    component.toggleDocumentType(22) // coverage
    component.filterRules = [{ rule_type: FILTER_DOCUMENT_TYPE, value: NEGATIVE_NULL_FILTER_VALUE.toString() }]
    expect(component.documentTypeSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.documentTypeSelectionModel.getExcludedItems()).toEqual([
      { id: NEGATIVE_NULL_FILTER_VALUE, name: 'Not set' },
    ])
  })

  it('should ingest filter rules for has any of document types', () => {
    component.filterRules = [
      { rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: '22' },
      { rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: '23' },
    ]
    expect(component.documentTypeSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.documentTypeSelectionModel.getSelectedItems()).toEqual(document_types)
    component.filterRules = [{ rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: null }]
  })

  it('should ingest filter rules for does not have any of document types', () => {
    component.filterRules = [
      { rule_type: FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE, value: '22' },
      { rule_type: FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE, value: '23' },
    ]
    expect(component.documentTypeSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.documentTypeSelectionModel.getExcludedItems()).toEqual(document_types)
    component.filterRules = [{ rule_type: FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE, value: null }]
  })

  it('should ingest filter rules for has storage path', () => {
    component.filterRules = [{ rule_type: FILTER_STORAGE_PATH, value: '32' }]
    expect(component.storagePathSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.storagePathSelectionModel.getSelectedItems()).toEqual([storage_paths[0]])
    component.toggleStoragePath(32) // coverage
    component.filterRules = [{ rule_type: FILTER_STORAGE_PATH, value: NEGATIVE_NULL_FILTER_VALUE.toString() }]
    expect(component.storagePathSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.storagePathSelectionModel.getExcludedItems()).toEqual([
      { id: NEGATIVE_NULL_FILTER_VALUE, name: 'Not set' },
    ])
  })

  it('should ingest filter rules for has any of storage paths', () => {
    component.filterRules = [
      { rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: '32' },
      { rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: '33' },
    ]
    expect(component.storagePathSelectionModel.logicalOperator).toEqual(LogicalOperator.Or)
    expect(component.storagePathSelectionModel.intersection).toEqual(Intersection.Include)
    expect(component.storagePathSelectionModel.getSelectedItems()).toEqual(storage_paths)
    component.filterRules = [{ rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: null }]
  })

  it('should ingest filter rules for does not have any of storage paths', () => {
    component.filterRules = [
      { rule_type: FILTER_DOES_NOT_HAVE_STORAGE_PATH, value: '32' },
      { rule_type: FILTER_DOES_NOT_HAVE_STORAGE_PATH, value: '33' },
    ]
    expect(component.storagePathSelectionModel.intersection).toEqual(Intersection.Exclude)
    expect(component.storagePathSelectionModel.getExcludedItems()).toEqual(storage_paths)
    component.filterRules = [{ rule_type: FILTER_DOES_NOT_HAVE_STORAGE_PATH, value: null }]
  })

  it('should ingest filter rules for custom fields all', () => {
    expect(component.customFieldQueriesModel.isEmpty()).toBeTruthy()
    component.filterRules = [{ rule_type: FILTER_HAS_CUSTOM_FIELDS_ALL, value: '42,43' }]
    expect(component.customFieldQueriesModel.queries[0].operator).toEqual(CustomFieldQueryLogicalOperator.And)
    expect(component.customFieldQueriesModel.queries[0].value.length).toEqual(2)
    expect((component.customFieldQueriesModel.queries[0].value[0] as CustomFieldQueryAtom).serialize()).toEqual([
      '42',
      CustomFieldQueryOperator.Exists,
      'true',
    ])
  })

  it('should ingest filter rules for has any custom fields', () => {
    component.filterRules = [{ rule_type: FILTER_HAS_CUSTOM_FIELDS_ANY, value: '42,43' }]
    expect(component.customFieldQueriesModel.queries[0].operator).toEqual(CustomFieldQueryLogicalOperator.Or)
    expect(component.customFieldQueriesModel.queries[0].value.length).toEqual(2)
    expect((component.customFieldQueriesModel.queries[0].value[0] as CustomFieldQueryAtom).serialize()).toEqual([
      '42',
      CustomFieldQueryOperator.Exists,
      'true',
    ])
  })

  it('should ingest filter rules for custom field queries', () => {
    component.filterRules = [
      { rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '["AND", [[42, "exists", "true"],[43, "exists", "true"]]]' },
    ]
    expect(component.customFieldQueriesModel.queries[0].operator).toEqual(CustomFieldQueryLogicalOperator.And)
    expect(component.customFieldQueriesModel.queries[0].value.length).toEqual(2)
    expect((component.customFieldQueriesModel.queries[0].value[0] as CustomFieldQueryAtom).serialize()).toEqual([
      42,
      CustomFieldQueryOperator.Exists,
      'true',
    ])
    // atom
    component.filterRules = [{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '[42, "exists", "true"]' }]
    expect(component.customFieldQueriesModel.queries[0].value.length).toEqual(1)
    expect((component.customFieldQueriesModel.queries[0].value[0] as CustomFieldQueryAtom).serialize()).toEqual([
      42,
      CustomFieldQueryOperator.Exists,
      'true',
    ])
    // invalid JSON is ignored
    component.filterRules = [{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '[42, ' }]
    expect(component.customFieldQueriesModel.isEmpty()).toBeTruthy()
  })

  it('should reflect ingested custom field query rules in the dropdown toggle', () => {
    expect(component.customFieldsDropdown.isActive).toBeFalsy()
    component.filterRules = [{ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '["OR",[[42,"exists","true"]]]' }]
    expect(component.customFieldsDropdown.isActive).toBeTruthy()
    component.filterRules = [{ rule_type: FILTER_HAS_TAGS_ALL, value: '19' }]
    expect(component.customFieldsDropdown.isActive).toBeFalsy()
  })

  it('should ingest filter rules for owner', () => {
    expect(component.permissionsSelectionModel.ownerFilter).toEqual(OwnerFilterType.NONE)
    component.filterRules = [{ rule_type: FILTER_OWNER, value: '100' }]
    expect(component.permissionsSelectionModel.ownerFilter).toEqual(OwnerFilterType.SELF)
    expect(component.permissionsSelectionModel.hideUnowned).toBeFalsy()
    expect(component.permissionsSelectionModel.userID).toEqual(100)
  })

  it('should reflect ingested owner filter rules in the dropdown toggle', () => {
    expect(component.permissionsFilter.isActive).toBeFalsy()
    component.filterRules = [{ rule_type: FILTER_OWNER, value: '100' }]
    expect(component.permissionsFilter.isActive).toBeTruthy()
    component.filterRules = []
    expect(component.permissionsFilter.isActive).toBeFalsy()
  })

  it('should ingest filter rules for owner is others', () => {
    component.filterRules = [{ rule_type: FILTER_OWNER_ANY, value: '50' }]
    expect(component.permissionsSelectionModel.ownerFilter).toEqual(OwnerFilterType.OTHERS)
    expect(component.permissionsSelectionModel.includeUsers).toContain(50)
  })

  it('should ingest filter rules for owner does not include others', () => {
    component.filterRules = [{ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '50' }]
    expect(component.permissionsSelectionModel.ownerFilter).toEqual(OwnerFilterType.NOT_SELF)
    expect(component.permissionsSelectionModel.excludeUsers).toContain(50)
  })

  it('should ingest filter rules for owner is null', () => {
    component.filterRules = [{ rule_type: FILTER_OWNER_ISNULL, value: 'true' }]
    expect(component.permissionsSelectionModel.ownerFilter).toEqual(OwnerFilterType.UNOWNED)
    expect(component.permissionsSelectionModel.hideUnowned).toBeFalsy()
  })

  it('should ingest filter rules for owner is not null', () => {
    component.filterRules = [{ rule_type: FILTER_OWNER_ISNULL, value: 'false' }]
    expect(component.permissionsSelectionModel.hideUnowned).toBeTruthy()
    component.filterRules = [{ rule_type: FILTER_OWNER_ISNULL, value: '0' }]
    expect(component.permissionsSelectionModel.hideUnowned).toBeTruthy()
  })

  it('should ingest filter rules for shared by me', () => {
    component.filterRules = [{ rule_type: FILTER_SHARED_BY_USER, value: '2' }]
    expect(component.permissionsSelectionModel.userID).toEqual(2)
  })

  // GET filterRules

  it('should convert user input to correct filter rules on text field search title + content', () => {
    component.textFilter = 'foo'
    tick(400) // debounce time
    expect(component.textFilter).toEqual('foo')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_SIMPLE_TEXT, value: 'foo' }])
    expect(setSearchQuery).toHaveBeenCalledWith('foo')
  })

  it('should emit filterRulesChange after the debounce (only for queries > 2 chars)', () => {
    const spy = vi.fn()
    component.onFilterRulesChange(spy)
    component.textFilter = 'fo'
    tick(400)
    expect(spy).not.toHaveBeenCalled()
    component.textFilter = 'foo'
    tick(399)
    expect(spy).not.toHaveBeenCalled()
    tick(1)
    expect(spy).toHaveBeenCalledWith([{ rule_type: FILTER_SIMPLE_TEXT, value: 'foo' }])
    component.textFilter = ''
    tick(400)
    expect(spy).toHaveBeenLastCalledWith([])
  })

  it('should convert user input to correct filter rules on text field search title only', () => {
    component.textFilter = 'foo'
    component.changeTextFilterTarget('title')
    tick(400)
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterTarget).toEqual('title')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_SIMPLE_TITLE, value: 'foo' }])
  })

  it('should convert user input to correct filter rules on text field search equals asn', () => {
    component.textFilter = '1234'
    component.changeTextFilterTarget(component.textFilterTargets[2].id) // ASN
    tick(400)
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('equals')
    expect(component.textFilter).toEqual('1234')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ASN, value: '1234' }])
  })

  it('should convert user input to correct filter rules on text field search greater than asn', () => {
    component.textFilter = '123'
    component.changeTextFilterTarget('asn')
    tick(400)
    component.setTextFilterModifier('greater')
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('greater')
    expect(component.textFilter).toEqual('123')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ASN_GT, value: '123' }])
  })

  it('should convert user input to correct filter rules on text field search less than asn', () => {
    component.textFilter = '999'
    component.changeTextFilterTarget('asn')
    tick(400)
    component.setTextFilterModifier('less')
    expect(component.textFilterModifier).toEqual('less')
    expect(component.textFilter).toEqual('999')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ASN_LT, value: '999' }])
  })

  it('should convert user input to correct filter rules on asn is null', () => {
    component.changeTextFilterTarget('asn')
    component.setTextFilterModifier('is null')
    expect(component.textFilterTarget).toEqual('asn')
    expect(component.textFilterModifier).toEqual('is null')
    expect(component.textFilterInputDisabled).toBeTruthy()
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ASN_ISNULL, value: 'true' }])
  })

  it('should convert user input to correct filter rules on asn is not null', () => {
    component.changeTextFilterTarget('asn')
    component.setTextFilterModifier('not null')
    expect(component.textFilterModifier).toEqual('not null')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ASN_ISNULL, value: 'false' }])
  })

  it('should convert user input to correct filter rules on mime type', () => {
    component.textFilter = 'pdf'
    component.changeTextFilterTarget(component.textFilterTargets[3].id)
    tick(400)
    expect(component.textFilterTarget).toEqual('mime-type')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_MIME_TYPE, value: 'pdf' }])
  })

  const targetIdByName = (name: string) => component.textFilterTargets.find((t) => t.name === name)?.id

  it('should convert duplicate target input to the correct filter rule', () => {
    component.changeTextFilterTarget(targetIdByName('Duplicates'))
    expect(component.textFilterTarget).toEqual('duplicates')
    expect(component.showTextFilterModifier).toBeTruthy()
    expect(component.textFilterModifiers.map((m) => m.id)).toEqual(['has-duplicates', 'does-not-have-duplicates'])
    expect(component.filterRules).toEqual([{ rule_type: FILTER_HAS_DUPLICATES, value: 'true' }])
    component.setTextFilterModifier('does-not-have-duplicates')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_HAS_DUPLICATES, value: 'false' }])
    // leaving the duplicates target resets the modifier
    component.changeTextFilterTarget('asn')
    expect(component.textFilterModifier).toEqual('equals')
  })

  it('should convert user input to correct filter rules on full text query', () => {
    component.textFilter = 'foo'
    component.changeTextFilterTarget(targetIdByName('Advanced search'))
    tick(400)
    expect(component.textFilterTarget).toEqual('fulltext-query')
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo' }])
  })

  it('should convert user input to correct filter rules on tag select not assigned', () => {
    component.tagDropdown.dropdownOpenChange(true)
    const tagButtons = component.tagDropdown.filteredItems
    component.tagSelectionModel.toggle(tagButtons[0].id) // (toggled)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_HAS_ANY_TAG, value: 'false' }])
  })

  it('should convert user input to correct filter rules on tag selections', () => {
    const rules: FilterRule[][] = []
    component.onFilterRulesChange((r) => rules.push(r))
    component.tagDropdown.dropdownOpenChange(true)
    const tagButtons = component.tagDropdown.filteredItems
    component.tagSelectionModel.toggle(tagButtons[1].id)
    component.tagSelectionModel.toggle(tagButtons[2].id)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_TAGS_ALL, value: tags[0].id.toString() },
      { rule_type: FILTER_HAS_TAGS_ALL, value: tags[1].id.toString() },
    ])
    expect(component.tagDropdown.showLogicalOperatorToggle).toBeTruthy()
    expect(component.tagDropdown.modifierToggleEnabled).toBeTruthy()

    component.tagDropdown.setLogicalOperator(LogicalOperator.Or)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_TAGS_ANY, value: tags[0].id.toString() },
      { rule_type: FILTER_HAS_TAGS_ANY, value: tags[1].id.toString() },
    ])

    component.tagDropdown.excludeClicked(tagButtons[2].id) // (exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_TAGS_ALL, value: tags[0].id.toString() },
      { rule_type: FILTER_DOES_NOT_HAVE_TAG, value: tags[1].id.toString() },
    ])
    expect(rules[rules.length - 1]).toEqual(component.filterRules)
  })

  it('should convert user input to correct filter rules on correspondent selections', () => {
    component.correspondentDropdown.dropdownOpenChange(true)
    const buttons = component.correspondentDropdown.filteredItems
    component.correspondentSelectionModel.toggle(buttons[1].id)
    component.correspondentSelectionModel.toggle(buttons[2].id)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: correspondents[0].id.toString() },
      { rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: correspondents[1].id.toString() },
    ])
    expect(component.correspondentDropdown.showIntersectionToggle).toBeTruthy()
    component.correspondentDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_DOES_NOT_HAVE_CORRESPONDENT, value: correspondents[0].id.toString() },
      { rule_type: FILTER_DOES_NOT_HAVE_CORRESPONDENT, value: correspondents[1].id.toString() },
    ])
  })

  it('should convert user input to correct filter rules on correspondent select not assigned', () => {
    component.correspondentDropdown.dropdownOpenChange(true)
    const notAssigned = component.correspondentDropdown.filteredItems[0]
    component.correspondentSelectionModel.toggle(notAssigned.id)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_CORRESPONDENT, value: null }])
    component.correspondentDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_CORRESPONDENT, value: NEGATIVE_NULL_FILTER_VALUE.toString() },
    ])
  })

  it('should convert user input to correct filter rules on document type selections', () => {
    component.documentTypeDropdown.dropdownOpenChange(true)
    const buttons = component.documentTypeDropdown.filteredItems
    component.documentTypeSelectionModel.toggle(buttons[1].id)
    component.documentTypeSelectionModel.toggle(buttons[2].id)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: document_types[0].id.toString() },
      { rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: document_types[1].id.toString() },
    ])
    component.documentTypeDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE, value: document_types[0].id.toString() },
      { rule_type: FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE, value: document_types[1].id.toString() },
    ])
  })

  it('should convert user input to correct filter rules on doc type select not assigned', () => {
    component.documentTypeDropdown.dropdownOpenChange(true)
    component.documentTypeSelectionModel.toggle(component.documentTypeDropdown.filteredItems[0].id)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_DOCUMENT_TYPE, value: null }])
    component.documentTypeDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_DOCUMENT_TYPE, value: NEGATIVE_NULL_FILTER_VALUE.toString() },
    ])
  })

  it('should convert user input to correct filter rules on storage path selections', () => {
    component.storagePathDropdown.dropdownOpenChange(true)
    const buttons = component.storagePathDropdown.filteredItems
    component.storagePathSelectionModel.toggle(buttons[1].id)
    component.storagePathSelectionModel.toggle(buttons[2].id)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: storage_paths[0].id.toString() },
      { rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: storage_paths[1].id.toString() },
    ])
    component.storagePathDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_DOES_NOT_HAVE_STORAGE_PATH, value: storage_paths[0].id.toString() },
      { rule_type: FILTER_DOES_NOT_HAVE_STORAGE_PATH, value: storage_paths[1].id.toString() },
    ])
  })

  it('should convert user input to correct filter rules on storage path select not assigned', () => {
    component.storagePathDropdown.dropdownOpenChange(true)
    component.storagePathSelectionModel.toggle(component.storagePathDropdown.filteredItems[0].id)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_STORAGE_PATH, value: null }])
    component.storagePathDropdown.setIntersection(Intersection.Exclude)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_STORAGE_PATH, value: NEGATIVE_NULL_FILTER_VALUE.toString() },
    ])
  })

  it('should convert user input to correct filter rules on custom field selections', () => {
    const spy = vi.fn()
    component.onFilterRulesChange(spy)
    component.customFieldsDropdown.onOpenChange(true)
    const expression = component.customFieldQueriesModel.queries[0] as CustomFieldQueryExpression
    const atom = expression.value[0] as CustomFieldQueryAtom
    atom.field = custom_fields[0].id
    expect(component.customFieldQueriesModel.queries[0].value.length).toEqual(1)
    const expected = [
      {
        rule_type: FILTER_CUSTOM_FIELDS_QUERY,
        value: JSON.stringify([CustomFieldQueryLogicalOperator.Or, [[custom_fields[0].id, 'exists', 'true']]]),
      },
    ]
    expect(component.filterRules).toEqual(expected)
    expect(spy).toHaveBeenLastCalledWith(expected)
  })

  it('should convert user input to correct filter rules on date created from', () => {
    component.dateCreatedFrom = '2023-05-14'
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_CREATED_FROM, value: '2023-05-14' }])
  })

  it('should emit rules after a debounced manual date change', () => {
    const spy = vi.fn()
    component.onFilterRulesChange(spy)
    component.dates.createdDateFrom = '2023-05-14'
    component.dates.onChangeDebounce()
    expect(spy).not.toHaveBeenCalled()
    tick(400)
    expect(spy).toHaveBeenCalledWith([{ rule_type: FILTER_CREATED_FROM, value: '2023-05-14' }])
  })

  it('should convert user input to correct filter rules on date created to', () => {
    component.dateCreatedTo = '2023-05-14'
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_CREATED_TO, value: '2023-05-14' }])
  })

  it('should convert user input to correct filter rules on date created with relative date', () => {
    component.dateCreatedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit() // (datesSet)
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' }])
  })

  it('should keep text filtering separate from date created relative date', () => {
    component.textFilter = 'foo'
    component.dateCreatedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_SIMPLE_TEXT, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' },
    ])
  })

  it('should keep text filtering separate from created and added relative dates', () => {
    component.textFilter = 'foo'
    component.dateCreatedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dateAddedRelativeDate = RelativeDate.WITHIN_1_MONTH
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_SIMPLE_TEXT, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now],added:[-1 month to now]' },
    ])
  })

  it('should keep legacy title filters as a title search when adding a created relative date', () => {
    component.filterRules = [{ rule_type: FILTER_TITLE, value: 'foo' }]
    component.dateCreatedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_SIMPLE_TITLE, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' },
    ])
  })

  it('should keep simple title filters as a title search when adding a created relative date', () => {
    component.filterRules = [{ rule_type: FILTER_SIMPLE_TITLE, value: 'foo' }]
    component.dateCreatedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_SIMPLE_TITLE, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-1 week to now]' },
    ])
  })

  it('should merge relative dates into an existing advanced search', () => {
    component.filterRules = [{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo' }]
    component.dates.onSetCreatedRelativeDate(RelativeDate.TODAY)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo,created:"today"' }])
  })

  it('should leave relative dates not in quick list intact', () => {
    component.textFilter = 'created:[-2 week to now]'
    component.changeTextFilterTarget(targetIdByName('Advanced search'))
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'created:[-2 week to now]' }])
    component.textFilter = 'added:[-2 month to now]'
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'added:[-2 month to now]' }])
  })

  it('should not update rules on debounce for advanced search (only on Enter)', () => {
    component.changeTextFilterTarget('fulltext-query')
    const spy = vi.fn()
    component.onFilterRulesChange(spy)
    component.textFilter = 'foo bar'
    tick(400)
    expect(spy).not.toHaveBeenCalled()
    component.textFilterKeydown('Enter', { inputValue: 'foo bar' })
    expect(spy).toHaveBeenCalledWith([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'foo bar' }])
  })

  it('should convert user input to correct filter rules on date added after', () => {
    component.dateAddedFrom = '2023-05-14'
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ADDED_FROM, value: '2023-05-14' }])
  })

  it('should convert user input to correct filter rules on date added before', () => {
    component.dateAddedTo = '2023-05-14'
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_ADDED_TO, value: '2023-05-14' }])
  })

  it('should convert user input to correct filter rules on date added with relative date', () => {
    component.dateAddedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_FULLTEXT_QUERY, value: 'added:[-1 week to now]' }])
  })

  it('should keep text filtering separate from date added relative date', () => {
    component.textFilter = 'foo'
    component.dateAddedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.dates.onChangeEmit()
    tick(400)
    expect(component.filterRules).toEqual([
      { rule_type: FILTER_SIMPLE_TEXT, value: 'foo' },
      { rule_type: FILTER_FULLTEXT_QUERY, value: 'added:[-1 week to now]' },
    ])
  })

  it('should convert user input to correct filter on permissions select my docs', () => {
    component.permissionsFilter.setFilter(OwnerFilterType.SELF)
    tick(400)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER, value: '1' }])
  })

  it('should convert user input to correct filter on permissions select shared with me', () => {
    component.permissionsFilter.setFilter(OwnerFilterType.NOT_SELF)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '1' }])
    component.permissionsSelectionModel.excludeUsers = [...component.permissionsSelectionModel.excludeUsers, 2]
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '1,2' }])
  })

  it('should convert user input to correct filter on permissions select shared by me', () => {
    component.permissionsFilter.setFilter(OwnerFilterType.SHARED_BY_ME)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_SHARED_BY_USER, value: '1' }])
  })

  it('should convert user input to correct filter on permissions select unowned', () => {
    component.permissionsFilter.setFilter(OwnerFilterType.UNOWNED)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER_ISNULL, value: 'true' }])
  })

  it('should convert user input to correct filter on permissions select others', () => {
    component.permissionsSelectionModel.ownerFilter = OwnerFilterType.OTHERS
    component.permissionsSelectionModel.includeUsers = [...component.permissionsSelectionModel.includeUsers, 3]
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER_ANY, value: '3' }])
  })

  it('should convert user input to correct filter on permissions hide unowned', () => {
    const spy = vi.fn()
    component.onFilterRulesChange(spy)
    component.permissionsFilter.setHideUnowned(true)
    expect(component.filterRules).toEqual([{ rule_type: FILTER_OWNER_ISNULL, value: 'false' }])
    expect(spy).toHaveBeenCalledWith([{ rule_type: FILTER_OWNER_ISNULL, value: 'false' }])
  })

  // The rest

  it('should support setting selection data', () => {
    component.selectionData = null
    expect(component.tagDocumentCounts).toBeNull()
    component.selectionData = {
      selected_storage_paths: [
        { id: 32, document_count: 1 },
        { id: 33, document_count: 0 },
      ],
      selected_correspondents: [
        { id: 12, document_count: 1 },
        { id: 13, document_count: 0 },
      ],
      selected_tags: [
        { id: 2, document_count: 1 },
        { id: 3, document_count: 0 },
      ],
      selected_document_types: [
        { id: 22, document_count: 1 },
        { id: 23, document_count: 0 },
      ],
      selected_custom_fields: [
        { id: 42, document_count: 1 },
        { id: 43, document_count: 0 },
      ],
    }
    expect(component.customFieldDocumentCounts).toHaveLength(2)
    expect(component.tagDropdown.getUpdatedDocumentCount(2)).toEqual(1)
    expect(component.correspondentDropdown.getUpdatedDocumentCount(13)).toEqual(0)
    expect(component.documentTypeDropdown.getUpdatedDocumentCount(22)).toEqual(1)
    expect(component.storagePathDropdown.getUpdatedDocumentCount(33)).toEqual(0)
    // counts reorder the items on the next apply() (dropdown open)
    component.correspondentDropdown.dropdownOpenChange(true)
    expect(strip(component.correspondentSelectionModel.items)).toEqual([
      { id: null, name: 'Not set' },
      ...correspondents,
    ])
  })

  it('should generate filter names', () => {
    const name = (rules: FilterRule[]) => {
      component.filterRules = rules
      return component.generateFilterName()
    }
    expect(name([{ rule_type: FILTER_HAS_CORRESPONDENT_ANY, value: '12' }])).toEqual(
      `Correspondent: ${correspondents[0].name}`
    )
    expect(name([{ rule_type: FILTER_CORRESPONDENT, value: null }])).toEqual('No correspondent')
    expect(name([{ rule_type: FILTER_HAS_DOCUMENT_TYPE_ANY, value: '22' }])).toEqual(
      `Document type: ${document_types[0].name}`
    )
    expect(name([{ rule_type: FILTER_DOCUMENT_TYPE, value: null }])).toEqual('No document type')
    expect(name([{ rule_type: FILTER_HAS_STORAGE_PATH_ANY, value: '32' }])).toEqual(
      `Storage path: ${storage_paths[0].name}`
    )
    expect(name([{ rule_type: FILTER_STORAGE_PATH, value: null }])).toEqual('No storage path')
    expect(name([{ rule_type: FILTER_HAS_TAGS_ALL, value: '2' }])).toEqual(`Tag: ${tags[0].name}`)
    expect(name([{ rule_type: FILTER_HAS_ANY_TAG, value: 'false' }])).toEqual('No tags')
    expect(name([{ rule_type: FILTER_HAS_DUPLICATES, value: 'true' }])).toEqual('Has duplicates')
    expect(name([{ rule_type: FILTER_HAS_DUPLICATES, value: 'false' }])).toEqual('No duplicates')
    expect(
      name([
        { rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: '["AND",[["42","exists","true"],["43","exists","true"]]]' },
      ])
    ).toEqual('Custom field condition')
    expect(name([{ rule_type: FILTER_SIMPLE_TITLE, value: 'foo' }])).toEqual('Title: foo')
    expect(name([{ rule_type: FILTER_TITLE_CONTENT, value: 'legacy foo' }])).toEqual('Title and content: legacy foo')
    expect(name([{ rule_type: FILTER_SIMPLE_TEXT, value: 'foo' }])).toEqual('Title and content: foo')
    expect(name([{ rule_type: FILTER_ASN, value: '1234' }])).toEqual('ASN: 1234')
    expect(name([{ rule_type: FILTER_OWNER, value: '1' }])).toEqual('Owner: 1')
    expect(name([{ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: '1' }])).toEqual('Owner is not: 1')
    expect(name([{ rule_type: FILTER_OWNER_ISNULL, value: 'true' }])).toEqual('No owner')
    expect(
      name([
        { rule_type: FILTER_HAS_TAGS_ANY, value: '2' },
        { rule_type: FILTER_HAS_TAGS_ANY, value: '3' },
      ])
    ).toEqual('')
  })

  it('should translate filter names and labels with the given translator', () => {
    const t = (k: string, p?: Record<string, any>) => `<${k.replace('{$PH}', String(p?.PH ?? ''))}>`
    const s = new FilterEditorState({ t, tags })
    s.filterRules = [{ rule_type: FILTER_HAS_TAGS_ALL, value: '2' }]
    expect(s.generateFilterName()).toEqual('<filter.tag>')
    expect(s.textFilterTargetName).toEqual('<filter.title_content>')
    expect(s.tagSelectionModel.items[0].name).toEqual('<common.not_assigned>')
    s.destroy()
  })

  it('should support resetting filter rules', () => {
    const rules = [
      { rule_type: FILTER_HAS_TAGS_ANY, value: '2' },
      { rule_type: FILTER_HAS_TAGS_ANY, value: '3' },
    ]
    component.unmodifiedFilterRules = rules
    component.filterRules = [
      { rule_type: FILTER_HAS_TAGS_ANY, value: '2' },
      { rule_type: FILTER_DOES_NOT_HAVE_TAG, value: '3' },
    ]
    expect(component.rulesModified).toBeTruthy()
    component.resetSelected()
    expect(component.filterRules).toEqual(rules)
    expect(component.rulesModified).toBeFalsy()
    expect(setSearchQuery).toHaveBeenCalledWith('')
  })

  it('should emit reset filter rules when resetting', () => {
    const rules = [{ rule_type: FILTER_HAS_TAGS_ANY, value: '2' }]
    component.unmodifiedFilterRules = rules
    component.filterRules = [{ rule_type: FILTER_DOES_NOT_HAVE_TAG, value: '2' }]
    const resetFilterRulesSpy = vi.spyOn(component.resetFilterRules, 'next')
    const filterRulesChangeSpy = vi.spyOn(component.filterRulesChange, 'next')
    component.resetSelected()
    expect(resetFilterRulesSpy).toHaveBeenCalledWith(rules)
    expect(filterRulesChangeSpy).not.toHaveBeenCalled()
  })

  it('should track rulesModified on updateRules', () => {
    component.unmodifiedFilterRules = []
    expect(component.rulesModified).toBeFalsy()
    component.toggleTag(2)
    expect(component.rulesModified).toBeTruthy()
    component.toggleTag(2)
    expect(component.rulesModified).toBeFalsy()
  })

  it('should support resetting text field', () => {
    component.textFilter = 'foo'
    component.resetTextField()
    expect(component.textFilter).toEqual('')
  })

  it('should support Enter / Esc key on text field', () => {
    component.textFilterKeydown('Enter', { inputValue: 'foo' })
    expect(component.textFilter).toEqual('foo')
    // Enter consumed by the typeahead is ignored
    component.textFilterKeydown('Enter', { inputValue: 'bar', defaultPrevented: true })
    expect(component.textFilter).toEqual('foo')
    expect(component.textFilterKeydown('Escape', { inputValue: 'foo bar' })).toBeUndefined()
    expect(component.textFilter).toEqual('')
    expect(component.textFilterKeydown('Escape', { inputValue: '' })).toEqual('blur')
  })

  it('should only dismiss open autocomplete suggestions on Escape, keeping the query', () => {
    component.textFilter = 'foo bar'
    expect(component.textFilterKeydown('Escape', { inputValue: 'foo bar', typeaheadOpen: true })).toEqual(
      'dismiss-typeahead'
    )
    expect(component.textFilter).toEqual('foo bar')
  })

  it('should adjust text filter targets if more like search', () => {
    component.textFilterTarget = 'fulltext-morelike'
    expect(component.textFilterTargets).toContainEqual({ id: 'fulltext-morelike', name: 'Similar to' })
    expect(component.textFilterTargetName).toEqual('Similar to')
    // leaving "more like" clears the text
    component._textFilter = 'Foo Bar'
    component.changeTextFilterTarget('title')
    expect(component.textFilter).toEqual('')
  })

  it('should hide deprecated custom fields target from default text filter targets', () => {
    expect(component.textFilterTargets).not.toContainEqual({ id: 'custom-fields', name: 'Custom fields (old)' })
    expect(component.textFilterTargets.map((t) => t.id)).toEqual([
      'title',
      'title-content',
      'asn',
      'mime-type',
      'duplicates',
      'fulltext-query',
    ])
  })

  it('should keep deprecated custom fields target available for legacy filters', () => {
    component.filterRules = [{ rule_type: FILTER_CUSTOM_FIELDS_TEXT, value: 'foo' }]
    tick()
    expect(component.textFilterTarget).toEqual('custom-fields')
    expect(component.textFilterTargets).toContainEqual({ id: 'custom-fields', name: 'Custom fields (old)' })
    expect(component.filterRules).toEqual([{ rule_type: FILTER_CUSTOM_FIELDS_TEXT, value: 'foo' }])
  })

  it('should call autocomplete endpoint on input', async () => {
    component.textFilterTarget = 'fulltext-query'
    autocomplete.mockResolvedValue(['hello'])
    expect(await component.searchAutoComplete('hello')).toEqual(['hello'])
    expect(autocomplete).toHaveBeenCalledWith('hello')
    await component.searchAutoComplete('hello world 1')
    expect(autocomplete).toHaveBeenCalledTimes(1) // term "1" is too short
    await component.searchAutoComplete('hello world')
    expect(autocomplete).toHaveBeenLastCalledWith('world')
    component.textFilterTarget = 'title'
    expect(await component.searchAutoComplete('hello')).toBeNull()
  })

  it('should handle autocomplete backend failure gracefully', async () => {
    component.textFilterTarget = 'fulltext-query'
    autocomplete.mockRejectedValue(new Error('autcomplete failed'))
    const result = await component.searchAutoComplete('hello')
    expect(autocomplete).toHaveBeenCalled()
    expect(result).toEqual([])
  })

  it('should support choosing a autocomplete item', () => {
    expect(component.textFilter).toBeNull()
    component.itemSelected('hello')
    expect(component.textFilter).toEqual('hello ')
    component.itemSelected('world')
    expect(component.textFilter).toEqual('hello world ')
  })

  it('should choose the active autocomplete item', () => {
    component.textFilterTarget = 'fulltext-query'
    component.textFilter = 'he'
    component.itemSelected('help')
    expect(component.textFilter).toEqual('help ')
  })

  it('should notify listeners on state changes and stop after destroy', () => {
    const spy = vi.fn()
    component.onChange(spy)
    component.filterRules = [{ rule_type: FILTER_HAS_TAGS_ALL, value: '2' }]
    expect(spy).toHaveBeenCalledTimes(1)
    component.toggleTag(3)
    expect(spy).toHaveBeenCalledTimes(2)
    component.destroy()
    const rulesSpy = vi.fn()
    component.onFilterRulesChange(rulesSpy)
    component.toggleTag(3)
    expect(rulesSpy).not.toHaveBeenCalled()
  })

  it('should set search query on init when a text filter exists', () => {
    component.filterRules = [{ rule_type: FILTER_SIMPLE_TEXT, value: 'foo' }]
    component.init()
    expect(setSearchQuery).toHaveBeenCalledWith('foo')
  })
})
