// Ported from utils/custom-field-query-element.spec.ts and
// custom-fields-query-dropdown.component.spec.ts (logic parts)
import { beforeEach, describe, expect, it, vi } from 'vitest'
import {
  CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP,
  CustomFieldDataType,
  CustomFieldQueryElementType,
  CustomFieldQueryLogicalOperator,
  CustomFieldQueryOperator,
  CustomFieldQueryOperatorGroups,
} from '../custom-field-query'
import {
  CustomFieldQueriesModel,
  CustomFieldQueryAtom,
  CustomFieldQueryElement,
  CustomFieldQueryExpression,
  CustomFieldsQueryDropdownState,
  type CustomFieldLike,
} from './custom-field-queries'

describe('CustomFieldQueryElement', () => {
  it('should initialize with correct type and id', () => {
    const element = new CustomFieldQueryElement(CustomFieldQueryElementType.Atom)
    expect(element.type).toBe(CustomFieldQueryElementType.Atom)
    expect(element.id).toBeDefined()
    expect(new CustomFieldQueryElement(CustomFieldQueryElementType.Atom).id).not.toEqual(element.id)
  })

  it('should trigger changed on operator change', () => {
    const element = new CustomFieldQueryElement(CustomFieldQueryElementType.Atom)
    const spy = vi.fn()
    element.onChange(spy)
    element.operator = CustomFieldQueryOperator.Exists
    expect(spy).toHaveBeenCalledWith(element)
  })

  it('should trigger changed subject on value change', () => {
    const element = new CustomFieldQueryElement(CustomFieldQueryElementType.Atom)
    const spy = vi.fn()
    element.onChange(spy)
    element.value = 'new value'
    expect(spy).toHaveBeenCalledWith(element)
  })

  it('should throw error on serialize call', () => {
    const element = new CustomFieldQueryElement(CustomFieldQueryElementType.Atom)
    expect(() => element.serialize()).toThrow('Implemented in subclass')
  })
})

describe('CustomFieldQueryAtom', () => {
  it('should initialize with correct field, operator, and value', () => {
    const atom = new CustomFieldQueryAtom([1, 'operator', 'value'])
    expect(atom.field).toBe(1)
    expect(atom.operator).toBe('operator')
    expect(atom.value).toBe('value')
  })

  it('should trigger changed subject on field change', () => {
    const atom = new CustomFieldQueryAtom()
    const spy = vi.fn()
    atom.onChange(spy)
    atom.field = 2
    expect(spy).toHaveBeenCalledWith(atom)
    expect(atom.field).toBe(2)
    atom.field = '3'
    expect(atom.field).toBe(3)
  })

  it('should set value to null if operator is not found in CUSTOM_FIELD_QUERY_VALUE_TYPES_BY_OPERATOR', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = 'nonexistent_operator'
    expect(atom.value).toBeNull()
  })

  it('should set value to empty string if new type is string', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = CustomFieldQueryOperator.IContains
    expect(atom.value).toBe('')
  })

  it('should set value to "true" if new type is boolean', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = CustomFieldQueryOperator.Exists
    expect(atom.value).toBe('true')
  })

  it('should set value to empty array if new type is array', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = CustomFieldQueryOperator.In
    expect(atom.value).toEqual([])
  })

  it('should try to set existing value to number if new type is number', () => {
    const atom = new CustomFieldQueryAtom()
    atom.value = '42'
    atom.operator = CustomFieldQueryOperator.GreaterThan
    expect(atom.value).toBe('42')

    // fallback to null if value is not parseable
    atom.value = 'not_a_number'
    atom.operator = CustomFieldQueryOperator.GreaterThan
    expect(atom.value).toBeNull()
  })

  it('should change boolean values to empty string if operator is not boolean', () => {
    const atom = new CustomFieldQueryAtom()
    atom.value = 'true'
    atom.operator = CustomFieldQueryOperator.Exact
    expect(atom.value).toBe('')
  })

  it('should serialize correctly', () => {
    const atom = new CustomFieldQueryAtom([1, 'operator', 'value'])
    expect(atom.serialize()).toEqual([1, 'operator', 'value'])
  })

  it('should emit changed on value change immediately', () => {
    const atom = new CustomFieldQueryAtom()
    const changeSpy = vi.spyOn(atom.changed, 'next')
    atom.value = 'new value'
    expect(changeSpy).toHaveBeenCalled()
  })

  it('should ignore duplicate array emissions', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = CustomFieldQueryOperator.In
    const changeSpy = vi.fn()
    atom.onChange(changeSpy)

    atom.value = [1, 2]
    expect(changeSpy).toHaveBeenCalledTimes(1)

    changeSpy.mockClear()
    atom.value = [1, 2]
    expect(changeSpy).not.toHaveBeenCalled()
  })

  it('should emit when array values differ while length matches', () => {
    const atom = new CustomFieldQueryAtom()
    atom.operator = CustomFieldQueryOperator.In
    const changeSpy = vi.fn()
    atom.onChange(changeSpy)

    atom.value = [1, 2]
    changeSpy.mockClear()
    atom.value = [1, 3]
    expect(changeSpy).toHaveBeenCalledTimes(1)
  })

  it('should emit one changed event when operator change coerces value', () => {
    const atom = new CustomFieldQueryAtom([1, CustomFieldQueryOperator.In, [1, 2]])
    const changeSpy = vi.fn()
    atom.onChange(changeSpy)

    atom.operator = CustomFieldQueryOperator.Exact

    expect(changeSpy).toHaveBeenCalledTimes(1)
    expect(atom.serialize()).toEqual([1, CustomFieldQueryOperator.Exact, ''])
  })
})

describe('CustomFieldQueryExpression', () => {
  it('should initialize with default operator and empty value', () => {
    const expression = new CustomFieldQueryExpression()
    expect(expression.operator).toBe(CustomFieldQueryLogicalOperator.Or)
    expect(expression.value).toEqual([])
  })

  it('should initialize with correct operator and value, propagate changes', () => {
    const expression = new CustomFieldQueryExpression([
      CustomFieldQueryLogicalOperator.And,
      [
        [1, 'exists', 'true'],
        [2, 'exists', 'true'],
      ],
    ])
    expect(expression.operator).toBe(CustomFieldQueryLogicalOperator.And)
    expect(expression.value.length).toBe(2)
    expect((expression.value[0] as CustomFieldQueryElement).depth).toBe(1)

    // propagate changes
    const expressionChangeSpy = vi.spyOn(expression.changed, 'next')
    ;(expression.value[0] as CustomFieldQueryAtom).emit()
    expect(expressionChangeSpy).toHaveBeenCalled()

    const expression2 = new CustomFieldQueryExpression([
      CustomFieldQueryLogicalOperator.Not,
      [[CustomFieldQueryLogicalOperator.Or, []]],
    ])
    const expressionChangeSpy2 = vi.spyOn(expression2.changed, 'next')
    ;(expression2.value[0] as CustomFieldQueryExpression).emit()
    expect(expressionChangeSpy2).toHaveBeenCalled()
  })

  it('should initialize with a sub-expression i.e. NOT', () => {
    const expression = new CustomFieldQueryExpression([
      CustomFieldQueryLogicalOperator.Not,
      [
        'AND',
        [
          [1, 'exists', 'true'],
          [2, 'exists', 'true'],
        ],
      ],
    ])
    expect(expression.value).toHaveLength(1)
    const changedSpy = vi.spyOn(expression.changed, 'next')
    ;(expression.value[0] as CustomFieldQueryExpression).emit()
    expect(changedSpy).toHaveBeenCalled()
  })

  it('should add atom correctly, propagate changes', () => {
    const expression = new CustomFieldQueryExpression()
    const atom = new CustomFieldQueryAtom([1, CustomFieldQueryOperator.Exists, 'true'])
    expression.addAtom(atom)
    expect(expression.value).toContain(atom)
    const changeSpy = vi.spyOn(expression.changed, 'next')
    atom.emit()
    expect(changeSpy).toHaveBeenCalled()
    expression.addAtom()
    expect((expression.value[1] as CustomFieldQueryAtom).serialize()).toEqual([null, 'exists', 'true'])
  })

  it('should add expression correctly, propagate changes', () => {
    const expression = new CustomFieldQueryExpression()
    const subExpression = new CustomFieldQueryExpression([CustomFieldQueryLogicalOperator.Or, []])
    expression.addExpression(subExpression)
    expect(expression.value).toContain(subExpression)
    const changeSpy = vi.spyOn(expression.changed, 'next')
    subExpression.emit()
    expect(changeSpy).toHaveBeenCalled()
    expression.addExpression()
    expect(expression.value).toHaveLength(2)
  })

  it('should serialize correctly', () => {
    const expression = new CustomFieldQueryExpression([CustomFieldQueryLogicalOperator.And, [[1, 'exists', 'true']]])
    expect(expression.serialize()).toEqual([CustomFieldQueryLogicalOperator.And, [[1, 'exists', 'true']]])
  })

  it('should serialize NOT expressions correctly', () => {
    const expression = new CustomFieldQueryExpression()
    expression.addExpression(
      new CustomFieldQueryExpression([
        CustomFieldQueryLogicalOperator.And,
        [
          [1, 'exists', 'true'],
          [2, 'exists', 'true'],
        ],
      ])
    )
    expression.operator = CustomFieldQueryLogicalOperator.Not
    const serialized = expression.serialize()
    expect(serialized[0]).toBe(CustomFieldQueryLogicalOperator.Not)
    expect(serialized[1][0]).toBe(CustomFieldQueryLogicalOperator.And)
    expect(serialized[1][1].length).toBe(2)
  })

  it('should be negatable if it has one child which is an expression', () => {
    const expression = new CustomFieldQueryExpression([
      CustomFieldQueryLogicalOperator.Not,
      [[CustomFieldQueryLogicalOperator.Or, []]],
    ])
    expect(expression.negatable).toBe(true)
  })
})

const customFields: CustomFieldLike[] = [
  { id: 1, name: 'Test Field', data_type: CustomFieldDataType.String, extra_data: {} },
  {
    id: 2,
    name: 'Test Select Field',
    data_type: CustomFieldDataType.Select,
    extra_data: {
      select_options: [
        { label: 'Option 1', id: 'abc-123' },
        { label: 'Option 2', id: 'def-456' },
      ],
    },
  },
  { id: 3, name: 'Test Monetary Field', data_type: CustomFieldDataType.Monetary, extra_data: { default_currency: 'EUR' } },
  { id: 4, name: 'Test Date Field', data_type: CustomFieldDataType.Date, extra_data: {} },
]

describe('CustomFieldsQueryDropdownState', () => {
  let component: CustomFieldsQueryDropdownState

  beforeEach(() => {
    component = new CustomFieldsQueryDropdownState(new CustomFieldQueriesModel(), { customFields, locale: 'de' })
    component.reset()
  })

  it('should initialize custom fields on creation', () => {
    expect(component.customFields).toEqual(customFields)
  })

  it('should add an expression when opened if queries are empty', () => {
    component.selectionModel.clear()
    component.onOpenChange(true)
    expect(component.selectionModel.queries.length).toBe(1)
    expect(component.shouldFocusFirstField).toBeTruthy()
  })

  it('should support reset the selection model', () => {
    component.selectionModel.addExpression()
    component.reset()
    expect(component.selectionModel.isEmpty()).toBeTruthy()
  })

  it('should get operators for a field', () => {
    component.customFields = [{ id: 1, name: 'Test Field', data_type: CustomFieldDataType.String, extra_data: {} }]
    const operators = component.getOperatorsForField(1)
    expect(operators.length).toEqual(
      [
        ...CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP[CustomFieldQueryOperatorGroups.Basic],
        ...CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP[CustomFieldQueryOperatorGroups.Exact],
        ...CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP[CustomFieldQueryOperatorGroups.String],
      ].length
    )
    expect(operators[0]).toEqual({ value: 'exists', label: 'filter.exists' })

    // Fallback to basic operators if field is not found
    const operators2 = component.getOperatorsForField(2)
    expect(operators2.length).toEqual(CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP[CustomFieldQueryOperatorGroups.Basic].length)
  })

  it('should get select options for a field', () => {
    component.customFields = [
      {
        id: 1,
        name: 'Test Field',
        data_type: CustomFieldDataType.Select,
        extra_data: {
          select_options: [
            { label: 'Option 1', id: 'abc-123' },
            { label: 'Option 2', id: 'def-456' },
          ],
        },
      },
    ]
    expect(component.getSelectOptionsForField(1)).toEqual([
      { label: 'Option 1', id: 'abc-123' },
      { label: 'Option 2', id: 'def-456' },
    ])
    expect(component.getSelectOptionsForField(2)).toEqual([])
  })

  it('should normalize localized monetary comparison values', () => {
    const atom = new CustomFieldQueryAtom([3, 'exact', null])
    component.setMonetaryValue(atom, '1.234,56')
    expect(atom.value).toEqual('1234.56')
  })

  it('should preserve API-formatted monetary comparison values', () => {
    const atom = new CustomFieldQueryAtom([3, 'exact', null])
    component.setMonetaryValue(atom, '1234.56')
    expect(atom.value).toEqual('1234.56')
  })

  it('should remove an element from the selection model', () => {
    const expression = new CustomFieldQueryExpression()
    const atom = new CustomFieldQueryAtom()
    ;(expression.value as CustomFieldQueryElement[]).push(atom)
    component.selectionModel.addExpression(expression)
    component.removeElement(atom)
    expect(component.selectionModel.isEmpty()).toBeTruthy()
    const expression2 = new CustomFieldQueryExpression([
      CustomFieldQueryLogicalOperator.And,
      [
        [1, 'icontains', 'test'],
        [2, 'icontains', 'test'],
      ],
    ])
    component.selectionModel.addExpression(expression2)
    component.removeElement(expression2)
    expect(component.selectionModel.isEmpty()).toBeTruthy()
  })

  it('should emit selectionModelChange when model changes', () => {
    const nextSpy = vi.spyOn(component.selectionModelChange, 'next')
    const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
    component.selectionModel.addAtom(atom)
    atom.emit()
    expect(nextSpy).toHaveBeenCalled()
  })

  it('should not emit selectionModelChange for invalid, non-empty models', () => {
    const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
    component.selectionModel.addAtom(atom)
    const nextSpy = vi.spyOn(component.selectionModelChange, 'next')
    component.selectionModel.addAtom(new CustomFieldQueryAtom([null, 'exists', 'true']))
    component.selectionModel.emit()
    expect(nextSpy).not.toHaveBeenCalled()
  })

  it('should complete selection model subscription when new selection model is set', () => {
    const completeSpy = vi.spyOn(component.selectionModel.changed, 'complete')
    component.selectionModel = new CustomFieldQueriesModel()
    expect(completeSpy).toHaveBeenCalled()
  })

  it('should support adding an atom', () => {
    const expression = new CustomFieldQueryExpression()
    component.addAtom(expression)
    expect(expression.value.length).toBe(1)
  })

  it('should support adding an expression', () => {
    const expression = new CustomFieldQueryExpression()
    component.addExpression(expression)
    expect(expression.value.length).toBe(1)
  })

  it('should support getting a custom field by ID', () => {
    expect(component.getCustomFieldByID(1)).toEqual(customFields[0])
  })

  it('should sanitize name from title', () => {
    component.title = 'Test Title'
    expect(component.name).toBe('test_title')
  })

  it('should add a default atom on open', () => {
    expect(component.selectionModel.queries.length).toBe(0)
    component.onOpenChange(true)
    expect(component.selectionModel.queries.length).toBe(1)
  })

  it('should build relative date values from amount and unit', () => {
    const atom = new CustomFieldQueryAtom([4, CustomFieldQueryOperator.GreaterThanOrEqual, '2026-10-22'])
    component.selectionModel.addAtom(atom)
    expect(component.hasDateValueRow(atom)).toBeTruthy()
    expect(component.getRelativeDate(atom)).toBeUndefined() // "fixed" mode

    component.toggleRelativeDate(atom) // "relative" radio
    expect(atom.value).toBeNull()
    component.updateRelativeDate(atom, { amount: 30 })
    expect(atom.serialize()).toEqual([4, 'gte', '30 days ago'])
    component.updateRelativeDate(atom, { unit: component.relativeDateUnits[6].id })
    expect(atom.serialize()).toEqual([4, 'gte', 'in 30 weeks'])
    component.updateRelativeDate(atom, { unit: component.relativeDateUnits[0].id })
    expect(atom.serialize()).toEqual([4, 'gte', 'today'])

    component.toggleRelativeDate(atom) // back to "fixed"
    expect(atom.value).toBeNull()
    expect(component.getRelativeDate(atom)).toBeUndefined()
  })

  it('should restore a saved relative date value', () => {
    const atom = new CustomFieldQueryAtom([4, CustomFieldQueryOperator.Exact, 'in 2 months'])
    component.selectionModel.addAtom(atom)
    expect(component.getRelativeDate(atom)).toEqual({ amount: 2, unit: 'in {n} months' })

    const today = new CustomFieldQueryAtom([4, CustomFieldQueryOperator.Exact, 'today'])
    expect(component.getRelativeDate(today)).toEqual({ amount: null, unit: 'today' })
  })

  it('should search fields and select options accent-insensitively', () => {
    expect(component.customFieldSearchFn('select', customFields[1])).toBeTruthy()
    expect(component.customFieldSearchFn('nope', customFields[1])).toBeFalsy()
    expect(component.selectOptionSearchFn('option 2', { id: 'x', label: 'Option 2' })).toBeTruthy()
  })

  describe('CustomFieldQueriesModel', () => {
    let model: CustomFieldQueriesModel

    beforeEach(() => {
      model = new CustomFieldQueriesModel()
    })

    it('should initialize with empty queries', () => {
      expect(model.queries).toEqual([])
    })

    it('should clear queries and fire event', () => {
      const nextSpy = vi.spyOn(model.changed, 'next')
      model.addExpression()
      model.clear()
      expect(model.queries).toEqual([])
      expect(nextSpy).toHaveBeenCalledWith(model)
    })

    it('should clear queries without firing event', () => {
      const nextSpy = vi.spyOn(model.changed, 'next')
      model.addExpression()
      model.clear(false)
      expect(model.queries).toEqual([])
      expect(nextSpy).not.toHaveBeenCalled()
    })

    it('should validate an empty model as invalid', () => {
      expect(model.isValid()).toBeFalsy()
    })

    it('should validate a model with valid expression as valid', () => {
      const expression = new CustomFieldQueryExpression()
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      const atom2 = new CustomFieldQueryAtom([2, 'icontains', 'test'])
      const expression2 = new CustomFieldQueryExpression()
      expression2.addAtom(atom)
      expression2.addAtom(atom2)
      expression.addExpression(expression2)
      model.addExpression(expression)
      expect(model.isValid()).toBeTruthy()
    })

    it('should validate a model with invalid expression as invalid', () => {
      const expression = new CustomFieldQueryExpression()
      model.addExpression(expression)
      expect(model.isValid()).toBeFalsy()
    })

    it('should validate an atom with in or contains operator', () => {
      const atom = new CustomFieldQueryAtom([1, 'in', '[1,2,3]'])
      expect((model as any)['validateAtom'].apply(null, [atom])).toBeTruthy()
      atom.operator = 'contains'
      atom.value = [1, 2, 3]
      expect((model as any)['validateAtom'].apply(null, [atom])).toBeTruthy()
      atom.value = null
      expect((model as any)['validateAtom'].apply(null, [atom])).toBeFalsy()
    })

    it('should check if model is empty', () => {
      expect(model.isEmpty()).toBeTruthy()
      model.addExpression()
      expect(model.isEmpty()).toBeTruthy()
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      model.addAtom(atom)
      expect(model.isEmpty()).toBeFalsy()
    })

    it('should add an atom to the model', () => {
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      model.addAtom(atom)
      expect(model.queries.length).toBe(1)
      expect((model.queries[0] as CustomFieldQueryExpression).value.length).toBe(1)
    })

    it('should add an expression to the model, propagate changes', () => {
      const expression = new CustomFieldQueryExpression()
      model.addExpression(expression)
      expect(model.queries.length).toBe(1)
      const expression2 = new CustomFieldQueryExpression([
        CustomFieldQueryLogicalOperator.And,
        [
          [1, 'icontains', 'test'],
          [2, 'icontains', 'test'],
        ],
      ])
      model.addExpression(expression2)
      const nextSpy = vi.spyOn(model.changed, 'next')
      expression2.emit()
      expect(nextSpy).toHaveBeenCalled()
    })

    it('should remove an element from the model', () => {
      const expression = new CustomFieldQueryExpression([
        CustomFieldQueryLogicalOperator.And,
        [
          [1, 'icontains', 'test'],
          [2, 'icontains', 'test'],
        ],
      ])
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      const expression2 = new CustomFieldQueryExpression([
        CustomFieldQueryLogicalOperator.And,
        [
          [3, 'icontains', 'test'],
          [4, 'icontains', 'test'],
        ],
      ])
      expression.addAtom(atom)
      expression2.addExpression(expression)
      model.addExpression(expression2)
      model.removeElement(atom)
      expect(model.queries.length).toBe(1)
      expect(expression.value).not.toContain(atom)
      model.removeElement(expression2)
      expect(model.queries.length).toBe(0)
    })

    it('should fire changed event when an atom changes', () => {
      const nextSpy = vi.spyOn(model.changed, 'next')
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      model.addAtom(atom)
      atom.emit()
      expect(nextSpy).toHaveBeenCalledWith(model)
    })

    it('should complete changed subject when element is removed', () => {
      const expression = new CustomFieldQueryExpression()
      const atom = new CustomFieldQueryAtom([1, 'icontains', 'test'])
      ;(expression.value as CustomFieldQueryElement[]).push(atom)
      model.addExpression(expression)
      const completeSpy = vi.spyOn(atom.changed, 'complete')
      model.removeElement(atom)
      expect(completeSpy).toHaveBeenCalled()
    })

    it('should subscribe to existing elements when queries are assigned', () => {
      const expression = new CustomFieldQueryExpression()
      const nextSpy = vi.spyOn(model.changed, 'next')
      model.queries = [expression]
      expression.emit()
      expect(nextSpy).toHaveBeenCalledWith(model)
    })

    it('should drop root subscriptions when queries are reassigned', () => {
      const expression = new CustomFieldQueryExpression()
      model.queries = [expression]
      model.queries = []
      const nextSpy = vi.spyOn(model.changed, 'next')
      expression.emit()
      expect(nextSpy).not.toHaveBeenCalled()
    })
  })
})
