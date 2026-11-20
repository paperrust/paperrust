// Port of utils/custom-field-query-element.ts and of CustomFieldQueriesModel + the pure logic
// of CustomFieldsQueryDropdownComponent
// (src-ui/src/app/components/common/custom-fields-query-dropdown/custom-fields-query-dropdown.component.ts).
// RxJS Subjects → Emitter (`onChange(fn)` / `emit()` / `changed.complete()`).

import {
  CUSTOM_FIELD_QUERY_OPERATOR_GROUPS_BY_TYPE,
  CUSTOM_FIELD_QUERY_OPERATOR_LABELS,
  CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP,
  CUSTOM_FIELD_QUERY_VALUE_TYPES_BY_OPERATOR,
  CustomFieldDataType,
  CustomFieldQueryElementType,
  CustomFieldQueryLogicalOperator,
  CustomFieldQueryOperator,
  CustomFieldQueryOperatorGroups,
} from '../custom-field-query'
import { Emitter, sanitizeName } from './emitter'
import { matchesSearchText } from './text-search'

export type CustomFieldQueryValue = string | string[] | number[] | CustomFieldQueryElement[]

let idCounter = 0
/** uuid v4 when available (crypto.randomUUID needs a secure context), else a unique fallback. */
function newId(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto
  if (c?.randomUUID) return c.randomUUID()
  return `cfq-${Date.now().toString(36)}-${(idCounter++).toString(36)}-${Math.random().toString(36).slice(2)}`
}

export class CustomFieldQueryElement {
  public readonly type: CustomFieldQueryElementType
  public readonly changed = new Emitter<CustomFieldQueryElement>()
  public depth: number = 0
  public id: string = newId()

  constructor(type: CustomFieldQueryElementType) {
    this.type = type
  }

  /** Subscribe to changes. Returns unsubscribe. */
  onChange(fn: (element: CustomFieldQueryElement) => void): () => void {
    return this.changed.subscribe(fn)
  }

  /** Angular `changed.next(this)`. */
  emit(): void {
    this.changed.next(this)
  }

  /** Angular `valueModelChanged` subscription (overridden by atoms). */
  protected valueModelChanged(_value: CustomFieldQueryValue): void {
    this.emit()
  }

  public serialize(): any {
    throw new Error('Implemented in subclass')
  }

  protected _operator: string = null
  public set operator(value: string) {
    this._operator = value
    this.emit()
  }
  public get operator(): string {
    return this._operator
  }

  protected _value: CustomFieldQueryValue = null
  public set value(value: CustomFieldQueryValue) {
    this._value = value
    this.valueModelChanged(value)
  }
  public get value(): CustomFieldQueryValue {
    return this._value
  }
}

const NOT_SET = Symbol('not-set')

export class CustomFieldQueryAtom extends CustomFieldQueryElement {
  protected _field: number
  // distinctUntilChanged state of the value stream
  private lastValue: CustomFieldQueryValue | typeof NOT_SET = NOT_SET

  set field(field: any) {
    this._field = parseInt(field, 10)
    this.emit()
  }
  get field(): number {
    return this._field
  }

  override set operator(operator: string) {
    const newTypes: string[] = CUSTOM_FIELD_QUERY_VALUE_TYPES_BY_OPERATOR[operator]?.split('|')
    if (!newTypes) {
      this._value = null
    } else {
      if (!newTypes.includes(typeof this.value)) {
        switch (newTypes[0]) {
          case 'string':
            this._value = ''
            break
          case 'boolean':
            this._value = 'true'
            break
          case 'array':
            this._value = []
            break
          case 'number': {
            const num = parseFloat(this.value as string)
            this._value = isNaN(num) ? null : num.toString()
            break
          }
        }
      } else if (['true', 'false'].includes(this.value as string) && newTypes.includes('string')) {
        this._value = ''
      }
    }
    super.operator = operator
  }

  override get operator(): string {
    return super.operator
  }

  constructor(queryArray: [number, string, string | string[] | number[]] = [null, null, null]) {
    super(CustomFieldQueryElementType.Atom)
    ;[this._field, this._operator, this._value] = queryArray
  }

  /** `distinctUntilChanged` with element-wise array comparison. */
  protected override valueModelChanged(current: CustomFieldQueryValue): void {
    const previous = this.lastValue
    this.lastValue = current
    if (previous !== NOT_SET && sameValue(previous, current)) return
    this.emit()
  }

  public override serialize(): [number, string, any] {
    return [this._field, this._operator, this._value]
  }
}

function sameValue(previous: CustomFieldQueryValue, current: CustomFieldQueryValue): boolean {
  if (Array.isArray(previous) && Array.isArray(current)) {
    if (previous.length !== current.length) {
      return false
    }
    for (let i = 0; i < previous.length; i++) {
      if (previous[i] !== current[i]) {
        return false
      }
    }
    return true
  }
  return previous === current
}

export class CustomFieldQueryExpression extends CustomFieldQueryElement {
  protected declare _value: string[] | number[] | CustomFieldQueryElement[]

  constructor(expressionArray: [CustomFieldQueryLogicalOperator | string, any[]] = [CustomFieldQueryLogicalOperator.Or, null]) {
    super(CustomFieldQueryElementType.Expression)
    let values: any[]
    ;[this._operator, values] = expressionArray
    if (!values || values.length === 0) {
      this._value = []
    } else if (values?.length > 0 && values[0] instanceof Array) {
      this._value = values.map((value) => {
        if (value.length === 3) {
          const atom = new CustomFieldQueryAtom(value)
          atom.depth = this.depth + 1
          atom.onChange(() => this.emit())
          return atom
        } else {
          const expression = new CustomFieldQueryExpression(value)
          expression.depth = this.depth + 1
          expression.onChange(() => this.emit())
          return expression
        }
      })
    } else {
      const expression = new CustomFieldQueryExpression(values as any)
      expression.depth = this.depth + 1
      expression.onChange(() => this.emit())
      this._value = [expression]
    }
  }

  public override serialize(): [string, any] {
    let value: any
    value = (this._value as CustomFieldQueryElement[]).map((element) => element.serialize())
    // If the expression is negated it should have only one child which is an expression
    if (this._operator === CustomFieldQueryLogicalOperator.Not && value.length === 1) {
      value = value[0]
    }
    return [this._operator, value]
  }

  public addAtom(
    atom: CustomFieldQueryAtom = new CustomFieldQueryAtom([null, CustomFieldQueryOperator.Exists, 'true'])
  ): void {
    atom.depth = this.depth + 1
    ;(this._value as CustomFieldQueryElement[]).push(atom)
    atom.onChange(() => this.emit())
  }

  public addExpression(expression: CustomFieldQueryExpression = new CustomFieldQueryExpression()): void {
    expression.depth = this.depth + 1
    ;(this._value as CustomFieldQueryElement[]).push(expression)
    expression.onChange(() => this.emit())
  }

  public get negatable(): boolean {
    return (
      this.value.length === 1 &&
      (this.value[0] as CustomFieldQueryElement).type === CustomFieldQueryElementType.Expression
    )
  }
}

// ---- CustomFieldQueriesModel -----------------------------------------------------------------

export class CustomFieldQueriesModel {
  private _queries: CustomFieldQueryElement[] = []
  private rootSubscriptions: Array<() => void> = []

  public readonly changed = new Emitter<CustomFieldQueriesModel>()

  /** Subscribe to changes. Returns unsubscribe. */
  onChange(fn: (model: CustomFieldQueriesModel) => void): () => void {
    return this.changed.subscribe(fn)
  }

  /** Angular `changed.next(this)`. */
  emit(): void {
    this.changed.next(this)
  }

  public get queries(): CustomFieldQueryElement[] {
    return this._queries
  }

  public set queries(value: CustomFieldQueryElement[]) {
    this.teardownRootSubscriptions()
    const queries = value ?? []
    for (const element of queries) {
      this.rootSubscriptions.push(element.onChange(() => this.emit()))
    }
    this._queries = queries
  }

  public clear(fireEvent = true): void {
    this.queries = []
    if (fireEvent) {
      this.emit()
    }
  }

  public isValid(): boolean {
    return this.queries.length > 0 && !!this.validateExpression(this.queries[0] as CustomFieldQueryExpression)
  }

  public isEmpty(): boolean {
    return this.queries.length === 0 || (this.queries.length === 1 && this.queries[0].value.length === 0)
  }

  private validateAtom(atom: CustomFieldQueryAtom): boolean {
    let valid = !!(atom.field && atom.operator && atom.value !== null)
    if (
      [CustomFieldQueryOperator.In as string, CustomFieldQueryOperator.Contains as string].includes(atom.operator) &&
      atom.value
    ) {
      valid = valid && atom.value.length > 0
    }
    return valid
  }

  private validateExpression(expression: CustomFieldQueryExpression): boolean {
    return (
      expression.operator &&
      expression.value.length > 0 &&
      (expression.value as CustomFieldQueryElement[]).every((e) =>
        e.type === CustomFieldQueryElementType.Atom
          ? this.validateAtom(e as CustomFieldQueryAtom)
          : this.validateExpression(e as CustomFieldQueryExpression)
      )
    ) as boolean
  }

  public addAtom(atom: CustomFieldQueryAtom): void {
    if (this.queries.length === 0) {
      this.addExpression()
    }
    ;(this.queries[0].value as CustomFieldQueryElement[]).push(atom)
    atom.onChange(() => {
      if (atom.field && atom.operator && atom.value) {
        this.emit()
      }
    })
  }

  public addExpression(expression: CustomFieldQueryExpression = new CustomFieldQueryExpression()): void {
    if (this.queries.length === 0) {
      this.queries = [expression]
      return
    }
    ;((this.queries[0] as CustomFieldQueryExpression).value as CustomFieldQueryElement[]).push(expression)
    expression.onChange(() => this.emit())
  }

  addInitialAtom(): void {
    this.addAtom(new CustomFieldQueryAtom([null, CustomFieldQueryOperator.Exists, 'true']))
  }

  private findElement(queryElement: CustomFieldQueryElement, elements: any[]): CustomFieldQueryElement {
    let foundElement: CustomFieldQueryElement
    for (let i = 0; i < elements.length; i++) {
      if (elements[i] === queryElement) {
        foundElement = elements.splice(i, 1)[0]
      } else if (elements[i].type === CustomFieldQueryElementType.Expression) {
        foundElement = this.findElement(queryElement, elements[i].value as CustomFieldQueryElement[])
      }
      if (foundElement) {
        break
      }
    }
    return foundElement
  }

  public removeElement(queryElement: CustomFieldQueryElement): void {
    let foundComponent: CustomFieldQueryElement
    for (let i = 0; i < this.queries.length; i++) {
      let query = this.queries[i]
      if (query === queryElement) {
        foundComponent = this.queries.splice(i, 1)[0]
        break
      } else if (query.type === CustomFieldQueryElementType.Expression) {
        foundComponent = this.findElement(queryElement, query.value as any[])
      }
    }
    if (foundComponent) {
      foundComponent.changed.complete()
      if (this.isEmpty()) {
        this.clear()
      }
      this.emit()
    }
  }

  private teardownRootSubscriptions(): void {
    for (const unsubscribe of this.rootSubscriptions) {
      unsubscribe()
    }
    this.rootSubscriptions = []
  }
}

// ---- CustomFieldsQueryDropdownComponent (pure logic) -----------------------------------------

export interface CustomFieldLike {
  id: number
  name: string
  data_type: CustomFieldDataType | string
  extra_data?: { select_options?: Array<{ label: string; id: string }>; [key: string]: any }
  [key: string]: any
}

export interface CustomFieldRelativeDate {
  /** The amount standing in for {n}; null for units without {n} or not yet entered. */
  amount: number | null
  unit: string
}

export function parseRelativeDate(value: string, unit: string): CustomFieldRelativeDate | undefined {
  const [prefix, suffix] = unit.split('{n}')
  if (suffix === undefined) {
    return value === unit ? { amount: null, unit } : undefined
  }
  if (!value.startsWith(prefix) || !value.endsWith(suffix)) return undefined
  const amount = value.slice(prefix.length, value.length - suffix.length)
  if (!amount || !Number.isInteger(+amount) || +amount < 0) return undefined
  return { amount: +amount, unit }
}

/** Relative date units; `label` is the i18n key (English source). */
export const RELATIVE_DATE_UNITS: ReadonlyArray<{ id: string; label: string }> = [
  { id: 'today', label: 'common.today' },
  { id: '{n} days ago', label: 'filter.days_ago' },
  { id: '{n} weeks ago', label: 'filter.weeks_ago' },
  { id: '{n} months ago', label: 'filter.months_ago' },
  { id: '{n} years ago', label: 'filter.years_ago' },
  { id: 'in {n} days', label: 'filter.days_now' },
  { id: 'in {n} weeks', label: 'filter.weeks_now' },
  { id: 'in {n} months', label: 'filter.months_now' },
  { id: 'in {n} years', label: 'filter.years_now' },
]

/** Decimal / group symbol of a locale (stand-in for Angular `getLocaleNumberSymbol`). */
export function localeNumberSymbols(locale: string): { decimal: string; group: string } {
  const parts = new Intl.NumberFormat(locale).formatToParts(1234567.5)
  return {
    decimal: parts.find((p) => p.type === 'decimal')?.value ?? '.',
    group: parts.find((p) => p.type === 'group')?.value ?? ',',
  }
}

/** Framework-free state of `<pngx-custom-fields-query-dropdown>`. */
export class CustomFieldsQueryDropdownState {
  /** Angular `selectionModelChange`: fired when the model changed and is empty or valid. */
  readonly selectionModelChange = new Emitter<CustomFieldQueriesModel>()

  title: string = null
  disabled = false
  customFields: CustomFieldLike[] = []
  locale: string

  private _selectionModel: CustomFieldQueriesModel
  private unsubscribeModel: () => void = () => {}
  private readonly relativeDates = new Map<string, CustomFieldRelativeDate>()

  readonly relativeDateUnits = RELATIVE_DATE_UNITS

  constructor(
    model: CustomFieldQueriesModel = new CustomFieldQueriesModel(),
    options: { customFields?: CustomFieldLike[]; locale?: string } = {}
  ) {
    this.customFields = options.customFields ?? []
    this.locale = options.locale ?? 'en-US'
    this.selectionModel = model
  }

  get selectionModel(): CustomFieldQueriesModel {
    return this._selectionModel
  }

  set selectionModel(model: CustomFieldQueriesModel) {
    if (this._selectionModel) {
      this.unsubscribeModel()
      this._selectionModel.changed.complete()
    }
    this.unsubscribeModel = model.onChange(() => this.onModelChange())
    this._selectionModel = model
  }

  /** Subscribe to `selectionModelChange`. Returns unsubscribe. */
  onChange(fn: (model: CustomFieldQueriesModel) => void): () => void {
    return this.selectionModelChange.subscribe(fn)
  }

  /** Angular also closes the dropdown when the model became empty – the UI does that on `isEmpty()`. */
  private onModelChange(): void {
    if (this._selectionModel.isEmpty() || this._selectionModel.isValid()) {
      this.selectionModelChange.next(this._selectionModel)
    }
  }

  get name(): string {
    return sanitizeName(this.title)
  }

  get isActive(): boolean {
    return this._selectionModel.isValid()
  }

  /** Call when the dropdown opens: adds an initial atom when empty. */
  onOpenChange(open: boolean): void {
    if (open && this._selectionModel.queries.length === 0) {
      this._selectionModel.addInitialAtom()
    }
  }

  /** True if the first field select should get focus after opening. */
  get shouldFocusFirstField(): boolean {
    const q = this._selectionModel.queries
    return q.length === 1 && ((q[0] as CustomFieldQueryExpression)?.value[0] as CustomFieldQueryAtom)?.field === null
  }

  getRelativeDate(atom: CustomFieldQueryAtom): CustomFieldRelativeDate | undefined {
    if (!this.relativeDates.has(atom.id) && typeof atom.value === 'string') {
      for (const { id } of this.relativeDateUnits) {
        const relativeDate = parseRelativeDate(atom.value, id)
        if (relativeDate) {
          this.relativeDates.set(atom.id, relativeDate)
          break
        }
      }
    }
    return this.relativeDates.get(atom.id)
  }

  hasDateValueRow(atom: CustomFieldQueryAtom): boolean {
    return (
      this.getCustomFieldByID(atom.field)?.data_type === CustomFieldDataType.Date &&
      atom.operator !== CustomFieldQueryOperator.Exists &&
      atom.operator !== CustomFieldQueryOperator.IsNull
    )
  }

  toggleRelativeDate(atom: CustomFieldQueryAtom): void {
    if (this.getRelativeDate(atom)) {
      this.relativeDates.delete(atom.id)
      atom.value = null
    } else {
      this.relativeDates.set(atom.id, { amount: null, unit: '{n} days ago' })
      this.updateRelativeDate(atom, {})
    }
  }

  updateRelativeDate(atom: CustomFieldQueryAtom, change: Partial<CustomFieldRelativeDate>): void {
    const { amount, unit } = Object.assign(this.relativeDates.get(atom.id), change)
    if (!unit.includes('{n}')) {
      atom.value = unit
    } else {
      atom.value = amount === null ? null : unit.replace('{n}', `${amount}`)
    }
  }

  customFieldSearchFn = (term: string, field: CustomFieldLike): boolean => matchesSearchText(field?.name, term)

  selectOptionSearchFn = (term: string, option: { id: string; label: string }): boolean =>
    matchesSearchText(option?.label, term)

  getCustomFieldByID(id: number): CustomFieldLike {
    return this.customFields.find((field) => field.id === id)
  }

  addAtom(expression: CustomFieldQueryExpression): void {
    expression.addAtom()
  }

  addExpression(expression: CustomFieldQueryExpression): void {
    expression.addExpression()
  }

  removeElement(element: CustomFieldQueryElement): void {
    this._selectionModel.removeElement(element)
  }

  reset(): void {
    this._selectionModel.clear(false)
    this._selectionModel.emit()
  }

  /** Operators for a field; `label` is the i18n key from CUSTOM_FIELD_QUERY_OPERATOR_LABELS. */
  getOperatorsForField(fieldID: number): Array<{ value: string; label: string }> {
    const field = this.customFields.find((f) => f.id === fieldID)
    const groups: CustomFieldQueryOperatorGroups[] = field
      ? CUSTOM_FIELD_QUERY_OPERATOR_GROUPS_BY_TYPE[field.data_type]
      : [CustomFieldQueryOperatorGroups.Basic]
    const operators: CustomFieldQueryOperator[] = groups.flatMap((group) => CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP[group])
    return operators.map((operator) => ({
      value: operator,
      label: CUSTOM_FIELD_QUERY_OPERATOR_LABELS[operator],
    }))
  }

  getSelectOptionsForField(fieldID: number): Array<{ label: string; id: string }> {
    const field = this.customFields.find((f) => f.id === fieldID)
    if (field) {
      return field.extra_data['select_options']
    }
    return []
  }

  setMonetaryValue(atom: CustomFieldQueryAtom, value: string): void {
    // Normalize the decimal symbol e.g. . vs , by locale
    const { decimal, group } = localeNumberSymbols(this.locale)
    if (decimal !== '.' && value.includes(decimal)) {
      value = value.split(group).join('').split(decimal).join('.')
    }
    atom.value = value
  }

  /** Release subscriptions (call on component destroy). */
  destroy(): void {
    this.unsubscribeModel()
  }
}
