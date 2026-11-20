// Port of the logic of FilterEditorComponent
// (src-ui/src/app/components/document-list/filter-editor/filter-editor.component.ts).
// Services become constructor options / setters; @Output()s and the template bindings
// between the editor and its dropdowns become Emitters that are wired here, so a Svelte
// UI only has to render the sub-states and call methods.

import { t as translate } from '../i18n'
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
import { filterRulesDiffer, type FilterRule } from '../filter-rules'
import { CustomFieldQueryLogicalOperator, CustomFieldQueryOperator } from '../custom-field-query'
import { msg } from '../i18n'
import {
  CustomFieldQueriesModel,
  CustomFieldQueryAtom,
  CustomFieldQueryExpression,
  CustomFieldsQueryDropdownState,
  type CustomFieldLike,
} from './custom-field-queries'
import { DatesModel, RelativeDate } from './dates'
import { Emitter, identityTranslator, type Translator } from './emitter'
import {
  OwnerFilterType,
  PermissionsFilterState,
  PermissionsSelectionModel,
  type PermissionsUser,
} from './permissions-model'
import {
  FilterableDropdownSelectionModel,
  FilterableDropdownState,
  Intersection,
  LogicalOperator,
  ToggleableItemState,
  flattenTags,
  type FilterableItem,
  type SelectionDataItem,
} from './selection-model'

const UNSET = Symbol('unset')

export const TEXT_FILTER_TARGET_TITLE = 'title'
export const TEXT_FILTER_TARGET_TITLE_CONTENT = 'title-content'
export const TEXT_FILTER_TARGET_ASN = 'asn'
export const TEXT_FILTER_TARGET_FULLTEXT_QUERY = 'fulltext-query'
export const TEXT_FILTER_TARGET_FULLTEXT_MORELIKE = 'fulltext-morelike'
export const TEXT_FILTER_TARGET_CUSTOM_FIELDS = 'custom-fields'
export const TEXT_FILTER_TARGET_MIME_TYPE = 'mime-type'
export const TEXT_FILTER_TARGET_DUPLICATES = 'duplicates'

export const TEXT_FILTER_MODIFIER_EQUALS = 'equals'
export const TEXT_FILTER_MODIFIER_NULL = 'is null'
export const TEXT_FILTER_MODIFIER_NOTNULL = 'not null'
export const TEXT_FILTER_MODIFIER_GT = 'greater'
export const TEXT_FILTER_MODIFIER_LT = 'less'
export const TEXT_FILTER_MODIFIER_HAS_DUPLICATES = 'has-duplicates'
export const TEXT_FILTER_MODIFIER_DOES_NOT_HAVE_DUPLICATES = 'does-not-have-duplicates'

const RELATIVE_DATE_QUERY_REGEXP_CREATED = /created:[\["]([^\]]+)[\]"]/g
const RELATIVE_DATE_QUERY_REGEXP_ADDED = /added:[\["]([^\]]+)[\]"]/g
export const RELATIVE_DATE_QUERYSTRINGS: ReadonlyArray<{
  relativeDate: RelativeDate
  dateQuery: string
  isRange?: boolean
}> = [
  { relativeDate: RelativeDate.WITHIN_1_WEEK, dateQuery: '-1 week to now', isRange: true },
  { relativeDate: RelativeDate.WITHIN_1_MONTH, dateQuery: '-1 month to now', isRange: true },
  { relativeDate: RelativeDate.WITHIN_3_MONTHS, dateQuery: '-3 month to now', isRange: true },
  { relativeDate: RelativeDate.WITHIN_1_YEAR, dateQuery: '-1 year to now', isRange: true },
  { relativeDate: RelativeDate.THIS_YEAR, dateQuery: 'this year' },
  { relativeDate: RelativeDate.THIS_MONTH, dateQuery: 'this month' },
  { relativeDate: RelativeDate.TODAY, dateQuery: 'today' },
  { relativeDate: RelativeDate.YESTERDAY, dateQuery: 'yesterday' },
  { relativeDate: RelativeDate.PREVIOUS_WEEK, dateQuery: 'previous week' },
  { relativeDate: RelativeDate.PREVIOUS_MONTH, dateQuery: 'previous month' },
  { relativeDate: RelativeDate.PREVIOUS_QUARTER, dateQuery: 'previous quarter' },
  { relativeDate: RelativeDate.PREVIOUS_YEAR, dateQuery: 'previous year' },
]

/** Option lists; `name` / `label` are i18n keys (English source). */
export interface TextFilterTargetOption {
  id: string
  name: string
}
export interface TextFilterModifierOption {
  id: string
  label: string
}

export const DEFAULT_TEXT_FILTER_TARGET_OPTIONS: ReadonlyArray<TextFilterTargetOption> = [
  { id: TEXT_FILTER_TARGET_TITLE, name: msg('common.title') },
  { id: TEXT_FILTER_TARGET_TITLE_CONTENT, name: msg('filter.title_content') },
  { id: TEXT_FILTER_TARGET_ASN, name: msg('common.asn') },
  { id: TEXT_FILTER_TARGET_MIME_TYPE, name: msg('filter.file_type') },
  { id: TEXT_FILTER_TARGET_DUPLICATES, name: msg('common.duplicates') },
  { id: TEXT_FILTER_TARGET_FULLTEXT_QUERY, name: msg('common.advanced_search') },
]

export const DEPRECATED_CUSTOM_FIELDS_TEXT_FILTER_TARGET_OPTION: TextFilterTargetOption = {
  // Kept only so legacy saved views can render and be edited away from, remove me eventually
  id: TEXT_FILTER_TARGET_CUSTOM_FIELDS,
  name: msg('filter.custom_fields_deprecated'),
}

export const TEXT_FILTER_TARGET_MORELIKE_OPTION: TextFilterTargetOption = {
  id: TEXT_FILTER_TARGET_FULLTEXT_MORELIKE,
  name: msg('filter.more_like'),
}

export const DEFAULT_TEXT_FILTER_MODIFIER_OPTIONS: ReadonlyArray<TextFilterModifierOption> = [
  { id: TEXT_FILTER_MODIFIER_EQUALS, label: msg('filter.equals') },
  { id: TEXT_FILTER_MODIFIER_NULL, label: msg('filter.empty') },
  { id: TEXT_FILTER_MODIFIER_NOTNULL, label: msg('filter.not_empty') },
  { id: TEXT_FILTER_MODIFIER_GT, label: msg('filter.greater') },
  { id: TEXT_FILTER_MODIFIER_LT, label: msg('filter.less') },
]

export const DUPLICATES_FILTER_MODIFIER_OPTIONS: ReadonlyArray<TextFilterModifierOption> = [
  { id: TEXT_FILTER_MODIFIER_HAS_DUPLICATES, label: msg('filter.exist') },
  { id: TEXT_FILTER_MODIFIER_DOES_NOT_HAVE_DUPLICATES, label: msg('filter.not_exist') },
]

/** i18n keys used by `generateFilterName()`. */
export const FILTER_NAME_KEYS = {
  correspondent: msg('filter.correspondent'),
  withoutCorrespondent: msg('common.without_correspondent'),
  documentType: msg('filter.document_type'),
  withoutDocumentType: msg('common.without_document_type'),
  storagePath: msg('filter.storage_path'),
  withoutStoragePath: msg('filter.without_storage_path'),
  tag: msg('filter.tag'),
  withoutAnyTag: msg('filter.without_any_tag'),
  withoutDuplicates: msg('filter.without_duplicates'),
  withDuplicates: msg('filter.duplicates'),
  customFieldsQuery: msg('filter.custom_fields_query'),
  title: msg('filter.title'),
  titleContent: msg('filter.title_content_2'),
  asn: msg('filter.asn'),
  owner: msg('filter.owner'),
  ownerNotIn: msg('filter.owner_not_in'),
  withoutOwner: msg('filter.without_owner'),
} as const

export interface SelectionData {
  selected_storage_paths: SelectionDataItem[]
  selected_correspondents: SelectionDataItem[]
  selected_tags: SelectionDataItem[]
  selected_document_types: SelectionDataItem[]
  selected_custom_fields: SelectionDataItem[]
}

export interface MoreLikeDocument {
  id?: number
  title: string
  [key: string]: any
}

export const TEXT_FILTER_DEBOUNCE_MS = 400
export const AUTOCOMPLETE_DEBOUNCE_MS = 200

export interface FilterEditorOptions {
  t?: Translator
  /** Id of the logged-in user (owner filters). */
  currentUserId?: number
  /** Users for the permissions dropdown labels (only when allowed to view users). */
  users?: PermissionsUser[]
  tags?: FilterableItem[]
  correspondents?: FilterableItem[]
  documentTypes?: FilterableItem[]
  storagePaths?: FilterableItem[]
  customFields?: CustomFieldLike[]
  /** Locale for monetary custom-field values. */
  locale?: string
  /** `documentService.get(id)` for "more like" (resolves the title shown in the text field). */
  getDocument?: (id: number) => Promise<MoreLikeDocument> | MoreLikeDocument
  /** `searchService.autocomplete(term)`. */
  autocomplete?: (term: string) => Promise<string[]> | string[]
  /** Side effect `documentService.searchQuery = text` (used for highlighting). */
  setSearchQuery?: (query: string) => void
}

/** Result of `textFilterKeydown()` that the UI has to perform. */
export type TextFilterKeydownAction = 'dismiss-typeahead' | 'blur' | void

export class FilterEditorState {
  /** Any state change (UI should re-render). */
  readonly changed = new Emitter<FilterEditorState>()
  /** Angular `filterRulesChange` output. */
  readonly filterRulesChange = new Emitter<FilterRule[]>()
  /** Angular `resetFilterRules` output. */
  readonly resetFilterRules = new Emitter<FilterRule[]>()

  private readonly t: Translator
  private readonly options: FilterEditorOptions

  customFields: CustomFieldLike[] = []

  tagDocumentCounts: SelectionDataItem[]
  correspondentDocumentCounts: SelectionDataItem[]
  documentTypeDocumentCounts: SelectionDataItem[]
  storagePathDocumentCounts: SelectionDataItem[]
  customFieldDocumentCounts: SelectionDataItem[]

  _textFilter = ''
  _moreLikeId: number
  _moreLikeDoc: MoreLikeDocument

  textFilterTarget: string = TEXT_FILTER_TARGET_TITLE_CONTENT
  textFilterModifier: string

  readonly tagSelectionModel: FilterableDropdownSelectionModel
  readonly correspondentSelectionModel: FilterableDropdownSelectionModel
  readonly documentTypeSelectionModel: FilterableDropdownSelectionModel
  readonly storagePathSelectionModel: FilterableDropdownSelectionModel
  readonly customFieldQueriesModel = new CustomFieldQueriesModel()
  readonly permissionsSelectionModel = new PermissionsSelectionModel()

  /** Dropdown states wired like the Angular template (allowSelectNone, opened → apply()). */
  readonly tagDropdown: FilterableDropdownState
  readonly correspondentDropdown: FilterableDropdownState
  readonly documentTypeDropdown: FilterableDropdownState
  readonly storagePathDropdown: FilterableDropdownState
  readonly customFieldsDropdown: CustomFieldsQueryDropdownState
  readonly dates = new DatesModel()
  readonly permissionsFilter: PermissionsFilterState

  rulesModified: boolean = false
  disabled = false

  _unmodifiedFilterRules: FilterRule[] = []
  _filterRules: FilterRule[] = []

  private textFilterTimer: ReturnType<typeof setTimeout> = null
  private lastDebouncedText: string | typeof UNSET = UNSET
  private disposers: Array<() => void> = []

  constructor(options: FilterEditorOptions = {}) {
    this.options = options
    this.t = options.t ?? translate

    this.tagSelectionModel = new FilterableDropdownSelectionModel(true, { t: this.t })
    this.correspondentSelectionModel = new FilterableDropdownSelectionModel(false, { t: this.t })
    this.documentTypeSelectionModel = new FilterableDropdownSelectionModel(false, { t: this.t })
    this.storagePathSelectionModel = new FilterableDropdownSelectionModel(false, { t: this.t })

    const dropdown = (model: FilterableDropdownSelectionModel, title: string) => {
      const d = new FilterableDropdownState(model)
      d.title = title
      d.allowSelectNone = true
      this.disposers.push(d.onChange(() => this.updateRules()))
      this.disposers.push(d.opened.subscribe(() => model.apply()))
      return d
    }
    this.tagDropdown = dropdown(this.tagSelectionModel, 'Tags')
    this.correspondentDropdown = dropdown(this.correspondentSelectionModel, 'Correspondent')
    this.documentTypeDropdown = dropdown(this.documentTypeSelectionModel, 'Document type')
    this.storagePathDropdown = dropdown(this.storagePathSelectionModel, 'Storage path')

    this.customFieldsDropdown = new CustomFieldsQueryDropdownState(this.customFieldQueriesModel, {
      customFields: options.customFields,
      locale: options.locale,
    })
    this.customFieldsDropdown.title = 'Custom fields'
    this.disposers.push(this.customFieldsDropdown.onChange(() => this.updateRules()))
    // changes that do not (yet) produce valid rules still need a re-render
    this.disposers.push(this.customFieldQueriesModel.onChange(() => this.notify()))

    this.dates.title = 'Dates'
    this.disposers.push(this.dates.onChange(() => this.updateRules()))

    this.permissionsFilter = new PermissionsFilterState(this.permissionsSelectionModel, {
      currentUserId: options.currentUserId,
      users: options.users,
      t: this.t,
    })
    this.permissionsFilter.title = 'Permissions'
    this.disposers.push(this.permissionsFilter.onChange(() => this.updateRules()))

    if (options.tags) this.setTags(options.tags)
    if (options.correspondents) this.setCorrespondents(options.correspondents)
    if (options.documentTypes) this.setDocumentTypes(options.documentTypes)
    if (options.storagePaths) this.setStoragePaths(options.storagePaths)
    if (options.customFields) this.setCustomFields(options.customFields)
  }

  // ---- listener API ----------------------------------------------------------------------------

  /** Subscribe to any state change. Returns unsubscribe. */
  onChange(fn: (state: FilterEditorState) => void): () => void {
    return this.changed.subscribe(fn)
  }

  /** Subscribe to rule changes made by the user (Angular `filterRulesChange`). */
  onFilterRulesChange(fn: (rules: FilterRule[]) => void): () => void {
    return this.filterRulesChange.subscribe(fn)
  }

  /** Subscribe to `resetSelected()` (Angular `resetFilterRules`). */
  onResetFilterRules(fn: (rules: FilterRule[]) => void): () => void {
    return this.resetFilterRules.subscribe(fn)
  }

  private notify(): void {
    this.changed.next(this)
  }

  /** Cancel timers and subscriptions. */
  destroy(): void {
    if (this.textFilterTimer) clearTimeout(this.textFilterTimer)
    this.textFilterTimer = null
    this.dates.destroy()
    for (const d of this.disposers) d()
    this.disposers = []
  }

  // ---- data from services ----------------------------------------------------------------------

  setTags(tags: FilterableItem[]): void {
    this.tagSelectionModel.items = flattenTags(tags)
    this.notify()
  }

  setCorrespondents(items: FilterableItem[]): void {
    this.correspondentSelectionModel.items = items
    this.notify()
  }

  setDocumentTypes(items: FilterableItem[]): void {
    this.documentTypeSelectionModel.items = items
    this.notify()
  }

  setStoragePaths(items: FilterableItem[]): void {
    this.storagePathSelectionModel.items = items
    this.notify()
  }

  setCustomFields(fields: CustomFieldLike[]): void {
    this.customFields = fields ?? []
    this.customFieldsDropdown.customFields = this.customFields
    this.notify()
  }

  setUsers(users: PermissionsUser[]): void {
    this.permissionsFilter.users = users ?? []
    this.notify()
  }

  setCurrentUserId(id: number): void {
    this.permissionsFilter.currentUserId = id
  }

  /** Angular ngOnInit: `if (this._textFilter) documentService.searchQuery = this._textFilter`. */
  init(): void {
    if (this._textFilter) this.options.setSearchQuery?.(this._textFilter)
  }

  // ---- generateFilterName ----------------------------------------------------------------------

  generateFilterName(): string {
    if (this.filterRules.length == 1) {
      let rule = this.filterRules[0]
      switch (rule.rule_type) {
        case FILTER_CORRESPONDENT:
        case FILTER_HAS_CORRESPONDENT_ANY:
          if (rule.value) {
            return this.t(FILTER_NAME_KEYS.correspondent, {
              PH: this.correspondentSelectionModel.items.find((c) => c.id == +rule.value)?.name,
            })
          } else {
            return this.t(FILTER_NAME_KEYS.withoutCorrespondent)
          }

        case FILTER_DOCUMENT_TYPE:
        case FILTER_HAS_DOCUMENT_TYPE_ANY:
          if (rule.value) {
            return this.t(FILTER_NAME_KEYS.documentType, {
              PH: this.documentTypeSelectionModel.items.find((dt) => dt.id == +rule.value)?.name,
            })
          } else {
            return this.t(FILTER_NAME_KEYS.withoutDocumentType)
          }

        case FILTER_STORAGE_PATH:
        case FILTER_HAS_STORAGE_PATH_ANY:
          if (rule.value) {
            return this.t(FILTER_NAME_KEYS.storagePath, {
              PH: this.storagePathSelectionModel.items.find((sp) => sp.id == +rule.value)?.name,
            })
          } else {
            return this.t(FILTER_NAME_KEYS.withoutStoragePath)
          }

        case FILTER_HAS_TAGS_ALL:
          return this.t(FILTER_NAME_KEYS.tag, {
            PH: this.tagSelectionModel.items.find((t) => t.id == +rule.value)?.name,
          })

        case FILTER_HAS_ANY_TAG:
          if (rule.value == 'false') {
            return this.t(FILTER_NAME_KEYS.withoutAnyTag)
          }
          break

        case FILTER_HAS_DUPLICATES:
          return rule.value == 'false'
            ? this.t(FILTER_NAME_KEYS.withoutDuplicates)
            : this.t(FILTER_NAME_KEYS.withDuplicates)

        case FILTER_CUSTOM_FIELDS_QUERY:
          return this.t(FILTER_NAME_KEYS.customFieldsQuery)

        case FILTER_TITLE:
        case FILTER_SIMPLE_TITLE:
          return this.t(FILTER_NAME_KEYS.title, { PH: rule.value })

        case FILTER_TITLE_CONTENT:
        case FILTER_SIMPLE_TEXT:
          return this.t(FILTER_NAME_KEYS.titleContent, { PH: rule.value })

        case FILTER_ASN:
          return this.t(FILTER_NAME_KEYS.asn, { PH: rule.value })

        case FILTER_OWNER:
          return this.t(FILTER_NAME_KEYS.owner, { PH: rule.value })

        case FILTER_OWNER_DOES_NOT_INCLUDE:
          return this.t(FILTER_NAME_KEYS.ownerNotIn, { PH: rule.value })

        case FILTER_OWNER_ISNULL:
          return this.t(FILTER_NAME_KEYS.withoutOwner)
      }
    }

    return ''
  }

  // ---- text filter -----------------------------------------------------------------------------

  /** Target options (translated). */
  get textFilterTargets(): TextFilterTargetOption[] {
    let targets: TextFilterTargetOption[] = [...DEFAULT_TEXT_FILTER_TARGET_OPTIONS]
    if (this.textFilterTarget == TEXT_FILTER_TARGET_FULLTEXT_MORELIKE) {
      targets = targets.concat([TEXT_FILTER_TARGET_MORELIKE_OPTION])
    }
    if (this.textFilterTarget == TEXT_FILTER_TARGET_CUSTOM_FIELDS) {
      targets = targets.concat([DEPRECATED_CUSTOM_FIELDS_TEXT_FILTER_TARGET_OPTION])
    }
    return targets.map((o) => ({ id: o.id, name: this.t(o.name) }))
  }

  get textFilterTargetName(): string {
    return this.textFilterTargets.find((t) => t.id == this.textFilterTarget)?.name
  }

  /** Modifier options (translated). */
  get textFilterModifiers(): TextFilterModifierOption[] {
    const options =
      this.textFilterTarget === TEXT_FILTER_TARGET_DUPLICATES
        ? DUPLICATES_FILTER_MODIFIER_OPTIONS
        : DEFAULT_TEXT_FILTER_MODIFIER_OPTIONS
    return options.map((o) => ({ id: o.id, label: this.t(o.label) }))
  }

  /** Modifier select is shown only for these targets. */
  get showTextFilterModifier(): boolean {
    return this.textFilterTarget === TEXT_FILTER_TARGET_ASN || this.textFilterTarget === TEXT_FILTER_TARGET_DUPLICATES
  }

  get textFilterModifierIsNull(): boolean {
    return [TEXT_FILTER_MODIFIER_NULL, TEXT_FILTER_MODIFIER_NOTNULL].includes(this.textFilterModifier)
  }

  get textFilterInputDisabled(): boolean {
    return this.textFilterModifierIsNull || this.textFilterTarget === TEXT_FILTER_TARGET_DUPLICATES
  }

  get textFilterInputReadonly(): boolean {
    return this.textFilterTarget === TEXT_FILTER_TARGET_FULLTEXT_MORELIKE
  }

  get textFilter(): string {
    return this.textFilterInputDisabled ? '' : this._textFilter
  }

  /** Typing in the text field: value is kept immediately, rules update after 400 ms. */
  set textFilter(value: string) {
    this._textFilter = value // set immediately to prevent loss of keystrokes
    this.queueTextFilterDebounce(value)
    this.notify()
  }

  private queueTextFilterDebounce(value: string): void {
    if (this.textFilterTimer) clearTimeout(this.textFilterTimer)
    this.textFilterTimer = setTimeout(() => {
      this.textFilterTimer = null
      // distinctUntilChanged()
      if (this.lastDebouncedText !== UNSET && this.lastDebouncedText === value) return
      this.lastDebouncedText = value
      // filter((query) => !query.length || query.length > 2)
      if (!(!value.length || value.length > 2)) return
      this.updateTextFilter(
        this._textFilter, // use the current value, not the debounced (possibly stale) one
        this.textFilterTarget !== TEXT_FILTER_TARGET_FULLTEXT_QUERY
      )
    }, TEXT_FILTER_DEBOUNCE_MS)
  }

  updateTextFilter(text: string, updateRules = true): void {
    this._textFilter = text
    if (updateRules) {
      this.options.setSearchQuery?.(text)
      this.updateRules()
    } else {
      this.notify()
    }
  }

  /**
   * Keydown in the text field. `inputValue` is the field's current value, `defaultPrevented`
   * whether the typeahead consumed Enter, `typeaheadOpen` whether suggestions are shown.
   */
  textFilterKeydown(
    key: string,
    opts: { inputValue?: string; defaultPrevented?: boolean; typeaheadOpen?: boolean } = {}
  ): TextFilterKeydownAction {
    if (key == 'Enter') {
      if (opts.defaultPrevented) {
        // NgbTypeahead calls preventDefault, so use that to detect if the Enter key was for the dropdown
        return
      }
      const filterString = opts.inputValue ?? ''
      if (filterString.length) {
        this.updateTextFilter(filterString)
      }
    } else if (key === 'Escape') {
      if (opts.typeaheadOpen) {
        // only dismiss the suggestions, so longer query can use Enter
        return 'dismiss-typeahead'
      }
      if (this._textFilter?.length) {
        this.resetTextField()
      } else {
        return 'blur'
      }
    }
  }

  resetTextField(): void {
    this.updateTextFilter('')
  }

  /** Target dropdown click (UI should focus the text field afterwards). */
  changeTextFilterTarget(target: string): void {
    if (this.textFilterTarget == TEXT_FILTER_TARGET_FULLTEXT_MORELIKE && target != TEXT_FILTER_TARGET_FULLTEXT_MORELIKE) {
      this._textFilter = ''
    }
    this.textFilterTarget = target
    if (target == TEXT_FILTER_TARGET_DUPLICATES) {
      this._textFilter = ''
      this.textFilterModifier = TEXT_FILTER_MODIFIER_HAS_DUPLICATES
    } else if (
      [TEXT_FILTER_MODIFIER_HAS_DUPLICATES, TEXT_FILTER_MODIFIER_DOES_NOT_HAVE_DUPLICATES].includes(
        this.textFilterModifier
      )
    ) {
      this.textFilterModifier = TEXT_FILTER_MODIFIER_EQUALS
    }
    this.updateRules()
  }

  /** Modifier select: `[(ngModel)]="textFilterModifier" (change)="textFilterModifierChange()"`. */
  setTextFilterModifier(modifier: string): void {
    this.textFilterModifier = modifier
    this.textFilterModifierChange()
  }

  textFilterModifierChange(): void {
    if (
      this.textFilterTarget == TEXT_FILTER_TARGET_DUPLICATES ||
      this.textFilterModifierIsNull ||
      ([TEXT_FILTER_MODIFIER_EQUALS, TEXT_FILTER_MODIFIER_GT, TEXT_FILTER_MODIFIER_LT].includes(
        this.textFilterModifier
      ) &&
        this._textFilter)
    ) {
      this.updateRules()
    } else {
      this.notify()
    }
  }

  // ---- autocomplete ----------------------------------------------------------------------------

  /**
   * Term to autocomplete for the typed text (last word), or null when autocomplete does not
   * apply (target is not advanced search). Debounce (200 ms) and distinct-ness are up to the UI.
   */
  autocompleteTerm(text: string): string | null {
    if (this.textFilterTarget !== TEXT_FILTER_TARGET_FULLTEXT_QUERY) return null
    return text.lastIndexOf(' ') != -1 ? text.substring(text.lastIndexOf(' ') + 1) : text
  }

  /** Suggestions for the typed text; `null` = no emission (wrong target). Errors → []. */
  async searchAutoComplete(text: string): Promise<string[] | null> {
    const term = this.autocompleteTerm(text)
    if (term === null) return null
    if (term.length < 2 || !this.options.autocomplete) return []
    try {
      return await this.options.autocomplete(term)
    } catch {
      return []
    }
  }

  /** Typeahead item chosen (Angular `itemSelected`; UI prevents the default input update). */
  itemSelected(item: string): void {
    let currentSearch: string = this._textFilter ?? ''
    let lastSpaceIndex = currentSearch.lastIndexOf(' ')
    if (lastSpaceIndex != -1) {
      currentSearch = currentSearch.substring(0, lastSpaceIndex + 1)
      currentSearch += item + ' '
    } else {
      currentSearch = item + ' '
    }
    this.updateTextFilter(currentSearch)
  }

  // ---- dates (single source of truth: this.dates) ---------------------------------------------

  get dateCreatedTo(): string {
    return this.dates.createdDateTo
  }
  set dateCreatedTo(v: string) {
    this.dates.createdDateTo = v
  }
  get dateCreatedFrom(): string {
    return this.dates.createdDateFrom
  }
  set dateCreatedFrom(v: string) {
    this.dates.createdDateFrom = v
  }
  get dateAddedTo(): string {
    return this.dates.addedDateTo
  }
  set dateAddedTo(v: string) {
    this.dates.addedDateTo = v
  }
  get dateAddedFrom(): string {
    return this.dates.addedDateFrom
  }
  set dateAddedFrom(v: string) {
    this.dates.addedDateFrom = v
  }
  get dateCreatedRelativeDate(): RelativeDate {
    return this.dates.createdRelativeDate
  }
  set dateCreatedRelativeDate(v: RelativeDate) {
    this.dates.createdRelativeDate = v
  }
  get dateAddedRelativeDate(): RelativeDate {
    return this.dates.addedRelativeDate
  }
  set dateAddedRelativeDate(v: RelativeDate) {
    this.dates.addedRelativeDate = v
  }

  // ---- rules ↔ models --------------------------------------------------------------------------

  set unmodifiedFilterRules(value: FilterRule[]) {
    this._unmodifiedFilterRules = value
    this.rulesModified = filterRulesDiffer(this._unmodifiedFilterRules, this._filterRules)
    this.notify()
  }

  get unmodifiedFilterRules(): FilterRule[] {
    return this._unmodifiedFilterRules
  }

  /** Load rules into the models (Angular `@Input() filterRules`). Does not emit filterRulesChange. */
  set filterRules(value: FilterRule[]) {
    if (value === this._filterRules) {
      return
    }
    this._filterRules = value

    this.documentTypeSelectionModel.clear(false)
    this.storagePathSelectionModel.clear(false)
    this.tagSelectionModel.clear(false)
    this.correspondentSelectionModel.clear(false)
    this.customFieldQueriesModel.clear(false)
    this._textFilter = null
    this._moreLikeId = null
    this.textFilterTarget = TEXT_FILTER_TARGET_TITLE_CONTENT
    this.dateAddedTo = null
    this.dateAddedFrom = null
    this.dateCreatedTo = null
    this.dateCreatedFrom = null
    this.dateCreatedRelativeDate = null
    this.dateAddedRelativeDate = null
    this.textFilterModifier = TEXT_FILTER_MODIFIER_EQUALS
    this.permissionsSelectionModel.clear()

    const perms = this.permissionsSelectionModel

    value.forEach((rule) => {
      switch (rule.rule_type) {
        case FILTER_TITLE:
        case FILTER_SIMPLE_TITLE:
          this._textFilter = rule.value
          this.textFilterTarget = TEXT_FILTER_TARGET_TITLE
          break
        case FILTER_TITLE_CONTENT:
        case FILTER_SIMPLE_TEXT:
          this._textFilter = rule.value
          this.textFilterTarget = TEXT_FILTER_TARGET_TITLE_CONTENT
          break
        case FILTER_ASN:
          this._textFilter = rule.value
          this.textFilterTarget = TEXT_FILTER_TARGET_ASN
          break
        case FILTER_CUSTOM_FIELDS_TEXT:
          this._textFilter = rule.value
          this.textFilterTarget = TEXT_FILTER_TARGET_CUSTOM_FIELDS
          break
        case FILTER_MIME_TYPE:
          this.textFilterTarget = TEXT_FILTER_TARGET_MIME_TYPE
          this._textFilter = rule.value
          break
        case FILTER_HAS_DUPLICATES:
          this.textFilterTarget = TEXT_FILTER_TARGET_DUPLICATES
          this.textFilterModifier =
            rule.value == 'false' || rule.value == '0'
              ? TEXT_FILTER_MODIFIER_DOES_NOT_HAVE_DUPLICATES
              : TEXT_FILTER_MODIFIER_HAS_DUPLICATES
          break
        case FILTER_FULLTEXT_QUERY: {
          let allQueryArgs = rule.value.split(',')
          let textQueryArgs: string[] = []
          allQueryArgs.forEach((arg) => {
            if (arg.match(RELATIVE_DATE_QUERY_REGEXP_CREATED)) {
              ;[...arg.matchAll(RELATIVE_DATE_QUERY_REGEXP_CREATED)].forEach((match) => {
                if (match[1]?.length) {
                  this.dateCreatedRelativeDate =
                    RELATIVE_DATE_QUERYSTRINGS.find((qS) => qS.dateQuery == match[1])?.relativeDate ?? null
                }
              })
              if (this.dateCreatedRelativeDate === null) textQueryArgs.push(arg) // relative query not in the quick list
            } else if (arg.match(RELATIVE_DATE_QUERY_REGEXP_ADDED)) {
              ;[...arg.matchAll(RELATIVE_DATE_QUERY_REGEXP_ADDED)].forEach((match) => {
                if (match[1]?.length) {
                  this.dateAddedRelativeDate =
                    RELATIVE_DATE_QUERYSTRINGS.find((qS) => qS.dateQuery == match[1])?.relativeDate ?? null
                }
              })
              if (this.dateAddedRelativeDate === null) textQueryArgs.push(arg) // relative query not in the quick list
            } else {
              textQueryArgs.push(arg)
            }
          })
          if (textQueryArgs.length) {
            this._textFilter = textQueryArgs.join(',')
            this.textFilterTarget = TEXT_FILTER_TARGET_FULLTEXT_QUERY
          }
          break
        }
        case FILTER_FULLTEXT_MORELIKE: {
          this._moreLikeId = +rule.value
          this.textFilterTarget = TEXT_FILTER_TARGET_FULLTEXT_MORELIKE
          const getDocument = this.options.getDocument
          if (getDocument) {
            const requestedId = this._moreLikeId
            Promise.resolve(getDocument(requestedId)).then(
              (result) => {
                if (!result) return
                this._moreLikeDoc = result
                this._textFilter = result.title
                this.notify()
              },
              () => {}
            )
          }
          break
        }
        case FILTER_CREATED_AFTER: {
          // Old rules require adjusting date by a day
          const createdAfter = new Date(rule.value)
          createdAfter.setDate(createdAfter.getDate() + 1)
          this.dateCreatedFrom = createdAfter.toISOString().split('T')[0]
          break
        }
        case FILTER_CREATED_BEFORE: {
          // Old rules require adjusting date by a day
          const createdBefore = new Date(rule.value)
          createdBefore.setDate(createdBefore.getDate() - 1)
          this.dateCreatedTo = createdBefore.toISOString().split('T')[0]
          break
        }
        case FILTER_ADDED_AFTER: {
          // Old rules require adjusting date by a day
          const addedAfter = new Date(rule.value)
          addedAfter.setDate(addedAfter.getDate() + 1)
          this.dateAddedFrom = addedAfter.toISOString().split('T')[0]
          break
        }
        case FILTER_ADDED_BEFORE: {
          // Old rules require adjusting date by a day
          const addedBefore = new Date(rule.value)
          addedBefore.setDate(addedBefore.getDate() - 1)
          this.dateAddedTo = addedBefore.toISOString().split('T')[0]
          break
        }
        case FILTER_CREATED_FROM:
          this.dateCreatedFrom = rule.value
          break
        case FILTER_CREATED_TO:
          this.dateCreatedTo = rule.value
          break
        case FILTER_ADDED_FROM:
          this.dateAddedFrom = rule.value
          break
        case FILTER_ADDED_TO:
          this.dateAddedTo = rule.value
          break
        case FILTER_HAS_TAGS_ALL:
          this.tagSelectionModel.logicalOperator = LogicalOperator.And
          this.tagSelectionModel.set(rule.value ? +rule.value : null, ToggleableItemState.Selected, false)
          break
        case FILTER_HAS_TAGS_ANY:
          this.tagSelectionModel.logicalOperator = LogicalOperator.Or
          this.tagSelectionModel.set(rule.value ? +rule.value : null, ToggleableItemState.Selected, false)
          break
        case FILTER_HAS_ANY_TAG:
          this.tagSelectionModel.set(null, ToggleableItemState.Selected, false)
          break
        case FILTER_DOES_NOT_HAVE_TAG:
          this.tagSelectionModel.set(rule.value ? +rule.value : null, ToggleableItemState.Excluded, false)
          break
        case FILTER_CORRESPONDENT:
          this.ingestSingleRule(this.correspondentSelectionModel, rule)
          break
        case FILTER_HAS_CORRESPONDENT_ANY:
          this.ingestAnyRule(this.correspondentSelectionModel, rule)
          break
        case FILTER_DOES_NOT_HAVE_CORRESPONDENT:
          this.ingestNotRule(this.correspondentSelectionModel, rule)
          break
        case FILTER_DOCUMENT_TYPE:
          this.ingestSingleRule(this.documentTypeSelectionModel, rule)
          break
        case FILTER_HAS_DOCUMENT_TYPE_ANY:
          this.ingestAnyRule(this.documentTypeSelectionModel, rule)
          break
        case FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE:
          this.ingestNotRule(this.documentTypeSelectionModel, rule)
          break
        case FILTER_STORAGE_PATH:
          this.ingestSingleRule(this.storagePathSelectionModel, rule)
          break
        case FILTER_HAS_STORAGE_PATH_ANY:
          this.ingestAnyRule(this.storagePathSelectionModel, rule)
          break
        case FILTER_DOES_NOT_HAVE_STORAGE_PATH:
          this.ingestNotRule(this.storagePathSelectionModel, rule)
          break
        case FILTER_CUSTOM_FIELDS_QUERY:
          try {
            const query = JSON.parse(rule.value)
            if (Array.isArray(query)) {
              if (query.length === 2) {
                // expression
                this.customFieldQueriesModel.addExpression(new CustomFieldQueryExpression(query as any))
              } else if (query.length === 3) {
                // atom
                this.customFieldQueriesModel.addAtom(new CustomFieldQueryAtom(query as any))
              }
            }
          } catch (e) {
            // error handled by list view service
          }
          break
        // Legacy custom field filters
        case FILTER_HAS_CUSTOM_FIELDS_ALL:
          this.customFieldQueriesModel.addExpression(
            new CustomFieldQueryExpression([
              CustomFieldQueryLogicalOperator.And,
              rule.value.split(',').map((id) => [id, CustomFieldQueryOperator.Exists, 'true']),
            ])
          )
          break
        case FILTER_HAS_CUSTOM_FIELDS_ANY:
          this.customFieldQueriesModel.addExpression(
            new CustomFieldQueryExpression([
              CustomFieldQueryLogicalOperator.Or,
              rule.value.split(',').map((id) => [id, CustomFieldQueryOperator.Exists, 'true']),
            ])
          )
          break
        case FILTER_ASN_ISNULL:
          this.textFilterTarget = TEXT_FILTER_TARGET_ASN
          this.textFilterModifier =
            rule.value == 'true' || rule.value == '1' ? TEXT_FILTER_MODIFIER_NULL : TEXT_FILTER_MODIFIER_NOTNULL
          break
        case FILTER_ASN_GT:
          this.textFilterTarget = TEXT_FILTER_TARGET_ASN
          this.textFilterModifier = TEXT_FILTER_MODIFIER_GT
          this._textFilter = rule.value
          break
        case FILTER_ASN_LT:
          this.textFilterTarget = TEXT_FILTER_TARGET_ASN
          this.textFilterModifier = TEXT_FILTER_MODIFIER_LT
          this._textFilter = rule.value
          break
        case FILTER_OWNER:
          perms.ownerFilter = OwnerFilterType.SELF
          perms.hideUnowned = false
          if (rule.value) perms.userID = Number.parseInt(rule.value, 10)
          break
        case FILTER_OWNER_ANY:
          perms.ownerFilter = OwnerFilterType.OTHERS
          if (rule.value) perms.includeUsers = [...perms.includeUsers, Number.parseInt(rule.value, 10)]
          break
        case FILTER_OWNER_DOES_NOT_INCLUDE:
          perms.ownerFilter = OwnerFilterType.NOT_SELF
          if (rule.value) perms.excludeUsers = [...perms.excludeUsers, Number.parseInt(rule.value, 10)]
          break
        case FILTER_SHARED_BY_USER:
          perms.ownerFilter = OwnerFilterType.SHARED_BY_ME
          if (rule.value) perms.userID = Number.parseInt(rule.value, 10)
          break
        case FILTER_OWNER_ISNULL:
          if (rule.value === 'true' || rule.value === '1') {
            perms.hideUnowned = false
            perms.ownerFilter = OwnerFilterType.UNOWNED
          } else {
            perms.hideUnowned = rule.value === 'false' || rule.value === '0'
            break
          }
      }
    })
    this.rulesModified = filterRulesDiffer(this._unmodifiedFilterRules, this._filterRules)
    this.notify()
  }

  private ingestSingleRule(model: FilterableDropdownSelectionModel, rule: FilterRule): void {
    model.intersection =
      rule.value == NEGATIVE_NULL_FILTER_VALUE.toString() ? Intersection.Exclude : Intersection.Include
    model.set(
      rule.value ? +rule.value : null,
      model.intersection == Intersection.Include ? ToggleableItemState.Selected : ToggleableItemState.Excluded,
      false
    )
  }

  private ingestAnyRule(model: FilterableDropdownSelectionModel, rule: FilterRule): void {
    model.logicalOperator = LogicalOperator.Or
    model.intersection = Intersection.Include
    model.set(rule.value ? +rule.value : null, ToggleableItemState.Selected, false)
  }

  private ingestNotRule(model: FilterableDropdownSelectionModel, rule: FilterRule): void {
    model.intersection = Intersection.Exclude
    model.set(rule.value ? +rule.value : null, ToggleableItemState.Excluded, false)
  }

  /** Rules computed from the current models (same order as Angular). */
  get filterRules(): FilterRule[] {
    let filterRules: FilterRule[] = []
    if (this._textFilter && this.textFilterTarget == TEXT_FILTER_TARGET_TITLE_CONTENT) {
      filterRules.push({ rule_type: FILTER_SIMPLE_TEXT, value: this._textFilter.trim() })
    }
    if (this.textFilterTarget == TEXT_FILTER_TARGET_DUPLICATES) {
      filterRules.push({
        rule_type: FILTER_HAS_DUPLICATES,
        value: (this.textFilterModifier == TEXT_FILTER_MODIFIER_HAS_DUPLICATES).toString(),
      })
    }
    if (this._textFilter && this.textFilterTarget == TEXT_FILTER_TARGET_TITLE) {
      filterRules.push({ rule_type: FILTER_SIMPLE_TITLE, value: this._textFilter })
    }
    if (this.textFilterTarget == TEXT_FILTER_TARGET_ASN) {
      if (this.textFilterModifier == TEXT_FILTER_MODIFIER_EQUALS && this._textFilter) {
        filterRules.push({ rule_type: FILTER_ASN, value: this._textFilter })
      } else if (this.textFilterModifierIsNull) {
        filterRules.push({
          rule_type: FILTER_ASN_ISNULL,
          value: (this.textFilterModifier == TEXT_FILTER_MODIFIER_NULL).toString(),
        })
      } else if (
        [TEXT_FILTER_MODIFIER_GT, TEXT_FILTER_MODIFIER_LT].includes(this.textFilterModifier) &&
        this._textFilter
      ) {
        filterRules.push({
          rule_type: this.textFilterModifier == TEXT_FILTER_MODIFIER_GT ? FILTER_ASN_GT : FILTER_ASN_LT,
          value: this._textFilter,
        })
      }
    }
    if (this._textFilter && this.textFilterTarget == TEXT_FILTER_TARGET_CUSTOM_FIELDS) {
      filterRules.push({ rule_type: FILTER_CUSTOM_FIELDS_TEXT, value: this._textFilter })
    }
    if (this._textFilter && this.textFilterTarget == TEXT_FILTER_TARGET_MIME_TYPE) {
      filterRules.push({ rule_type: FILTER_MIME_TYPE, value: this._textFilter })
    }
    if (this._textFilter && this.textFilterTarget == TEXT_FILTER_TARGET_FULLTEXT_QUERY) {
      filterRules.push({ rule_type: FILTER_FULLTEXT_QUERY, value: this._textFilter.trim() })
    }
    if (this._moreLikeId && this.textFilterTarget == TEXT_FILTER_TARGET_FULLTEXT_MORELIKE) {
      filterRules.push({ rule_type: FILTER_FULLTEXT_MORELIKE, value: this._moreLikeId.toString() })
    }
    if (this.tagSelectionModel.isNoneSelected()) {
      filterRules.push({ rule_type: FILTER_HAS_ANY_TAG, value: 'false' })
    } else {
      const tagFilterType =
        this.tagSelectionModel.logicalOperator == LogicalOperator.And ? FILTER_HAS_TAGS_ALL : FILTER_HAS_TAGS_ANY
      this.tagSelectionModel
        .getSelectedItems()
        .filter((tag) => tag.id)
        .forEach((tag) => {
          filterRules.push({ rule_type: tagFilterType, value: tag.id?.toString() })
        })
      this.tagSelectionModel
        .getExcludedItems()
        .filter((tag) => tag.id)
        .forEach((tag) => {
          filterRules.push({ rule_type: FILTER_DOES_NOT_HAVE_TAG, value: tag.id?.toString() })
        })
    }
    this.pushManyToOneRules(
      filterRules,
      this.correspondentSelectionModel,
      FILTER_CORRESPONDENT,
      FILTER_HAS_CORRESPONDENT_ANY,
      FILTER_DOES_NOT_HAVE_CORRESPONDENT
    )
    this.pushManyToOneRules(
      filterRules,
      this.documentTypeSelectionModel,
      FILTER_DOCUMENT_TYPE,
      FILTER_HAS_DOCUMENT_TYPE_ANY,
      FILTER_DOES_NOT_HAVE_DOCUMENT_TYPE
    )
    this.pushManyToOneRules(
      filterRules,
      this.storagePathSelectionModel,
      FILTER_STORAGE_PATH,
      FILTER_HAS_STORAGE_PATH_ANY,
      FILTER_DOES_NOT_HAVE_STORAGE_PATH
    )
    let queries = this.customFieldQueriesModel.queries.map((query) => query.serialize())
    if (queries.length > 0 && this.customFieldQueriesModel.isValid()) {
      filterRules.push({ rule_type: FILTER_CUSTOM_FIELDS_QUERY, value: JSON.stringify(queries[0]) })
    }
    if (this.dateCreatedTo) {
      filterRules.push({ rule_type: FILTER_CREATED_TO, value: this.dateCreatedTo })
    }
    if (this.dateCreatedFrom) {
      filterRules.push({ rule_type: FILTER_CREATED_FROM, value: this.dateCreatedFrom })
    }
    if (this.dateAddedTo) {
      filterRules.push({ rule_type: FILTER_ADDED_TO, value: this.dateAddedTo })
    }
    if (this.dateAddedFrom) {
      filterRules.push({ rule_type: FILTER_ADDED_FROM, value: this.dateAddedFrom })
    }
    if (this.dateAddedRelativeDate !== null || this.dateCreatedRelativeDate !== null) {
      let existingRule = filterRules.find((fr) => fr.rule_type == FILTER_FULLTEXT_QUERY)

      let queryArgs = existingRule?.value.split(',') ?? []
      if (this.dateCreatedRelativeDate !== null) {
        const rd = RELATIVE_DATE_QUERYSTRINGS.find((qS) => qS.relativeDate == this.dateCreatedRelativeDate)
        queryArgs = queryArgs.filter((arg) => !arg.match(RELATIVE_DATE_QUERY_REGEXP_CREATED))
        queryArgs.push(`created:${rd.isRange ? `[${rd.dateQuery}]` : `"${rd.dateQuery}"`}`)
      }
      if (this.dateAddedRelativeDate !== null) {
        const rd = RELATIVE_DATE_QUERYSTRINGS.find((qS) => qS.relativeDate == this.dateAddedRelativeDate)
        queryArgs = queryArgs.filter((arg) => !arg.match(RELATIVE_DATE_QUERY_REGEXP_ADDED))
        queryArgs.push(`added:${rd.isRange ? `[${rd.dateQuery}]` : `"${rd.dateQuery}"`}`)
      }

      if (existingRule) {
        existingRule.value = queryArgs.join(',')
      } else {
        filterRules.push({ rule_type: FILTER_FULLTEXT_QUERY, value: queryArgs.join(',') })
      }
    }
    const perms = this.permissionsSelectionModel
    if (perms.ownerFilter == OwnerFilterType.SELF) {
      filterRules.push({ rule_type: FILTER_OWNER, value: perms.userID.toString() })
    } else if (perms.ownerFilter == OwnerFilterType.NOT_SELF) {
      filterRules.push({ rule_type: FILTER_OWNER_DOES_NOT_INCLUDE, value: perms.excludeUsers?.join(',') })
    } else if (perms.ownerFilter == OwnerFilterType.OTHERS) {
      filterRules.push({ rule_type: FILTER_OWNER_ANY, value: perms.includeUsers?.join(',') })
    } else if (perms.ownerFilter == OwnerFilterType.SHARED_BY_ME) {
      filterRules.push({ rule_type: FILTER_SHARED_BY_USER, value: perms.userID.toString() })
    } else if (perms.ownerFilter == OwnerFilterType.UNOWNED) {
      filterRules.push({ rule_type: FILTER_OWNER_ISNULL, value: 'true' })
    }

    if (perms.hideUnowned) {
      filterRules.push({ rule_type: FILTER_OWNER_ISNULL, value: 'false' })
    }
    return filterRules
  }

  /** Correspondent / document type / storage path block of the Angular getter. */
  private pushManyToOneRules(
    filterRules: FilterRule[],
    model: FilterableDropdownSelectionModel,
    singleType: number,
    anyType: number,
    notType: number
  ): void {
    if (model.isNoneSelected() && model.intersection == Intersection.Include) {
      filterRules.push({ rule_type: singleType, value: null })
    } else {
      if (model.isNoneSelected() && model.intersection == Intersection.Exclude) {
        filterRules.push({ rule_type: singleType, value: NEGATIVE_NULL_FILTER_VALUE.toString() })
      }
      model.getSelectedItems().forEach((item) => {
        filterRules.push({ rule_type: anyType, value: item.id?.toString() })
      })
      model
        .getExcludedItems()
        .filter((item) => item.id > 0)
        .forEach((item) => {
          filterRules.push({ rule_type: notType, value: item.id?.toString() })
        })
    }
  }

  /** Angular `@Input() selectionData`: stores the counts and passes them to the dropdowns. */
  set selectionData(selectionData: SelectionData) {
    this.tagDocumentCounts = selectionData?.selected_tags ?? null
    this.documentTypeDocumentCounts = selectionData?.selected_document_types ?? null
    this.correspondentDocumentCounts = selectionData?.selected_correspondents ?? null
    this.storagePathDocumentCounts = selectionData?.selected_storage_paths ?? null
    this.customFieldDocumentCounts = selectionData?.selected_custom_fields ?? null
    // template: [documentCounts]="…DocumentCounts" (the dropdown ignores null)
    this.tagDropdown.documentCounts = this.tagDocumentCounts
    this.correspondentDropdown.documentCounts = this.correspondentDocumentCounts
    this.documentTypeDropdown.documentCounts = this.documentTypeDocumentCounts
    this.storagePathDropdown.documentCounts = this.storagePathDocumentCounts
    this.notify()
  }

  /** Recompute rules from the models and emit `filterRulesChange`. */
  updateRules(): void {
    const updatedRules = this.filterRules
    this._filterRules = updatedRules
    this.rulesModified = filterRulesDiffer(this._unmodifiedFilterRules, updatedRules)
    this.filterRulesChange.next(updatedRules)
    this.notify()
  }

  resetSelected(): void {
    this.textFilterTarget = TEXT_FILTER_TARGET_TITLE_CONTENT
    this.options.setSearchQuery?.('')
    this.filterRules = this._unmodifiedFilterRules
    this.resetFilterRules.next(this.filterRules)
    this.notify()
  }

  toggleTag(tagId: number): void {
    this.tagSelectionModel.toggle(tagId)
  }

  toggleCorrespondent(correspondentId: number): void {
    this.correspondentSelectionModel.toggle(correspondentId)
  }

  toggleDocumentType(documentTypeId: number): void {
    this.documentTypeSelectionModel.toggle(documentTypeId)
  }

  toggleStoragePath(storagePathID: number): void {
    this.storagePathSelectionModel.toggle(storagePathID)
  }

  onTagsDropdownOpen(): void {
    this.tagSelectionModel.apply()
  }

  onCorrespondentDropdownOpen(): void {
    this.correspondentSelectionModel.apply()
  }

  onDocumentTypeDropdownOpen(): void {
    this.documentTypeSelectionModel.apply()
  }

  onStoragePathDropdownOpen(): void {
    this.storagePathSelectionModel.apply()
  }
}

