// Ported from dates-dropdown.component.spec.ts (logic parts)
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  DatesModel,
  RELATIVE_DATE_LABELS,
  RelativeDate,
  getRelativeDates,
  isAllowedDateInputKey,
  todayISO,
  type DateSelection,
} from './dates'

describe('DatesModel', () => {
  let component: DatesModel

  beforeEach(() => {
    component = new DatesModel()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('should support date input, emit change after debounce', () => {
    vi.useFakeTimers()
    let result: DateSelection = null
    component.onChange((d) => (result = d))
    component.createdDateFrom = '2023-05-30'
    component.onChangeDebounce()
    expect(result).toBeNull()
    vi.advanceTimersByTime(500)
    expect(result).not.toBeNull()
    expect(result.createdFrom).toEqual('2023-05-30')
  })

  it('should support relative dates', () => {
    vi.useFakeTimers()
    let result: DateSelection
    component.onChange((date) => (result = date))
    component.onSetCreatedRelativeDate(RelativeDate.WITHIN_1_WEEK)
    component.onSetAddedRelativeDate(RelativeDate.WITHIN_1_WEEK)
    vi.advanceTimersByTime(500)
    expect(result).toEqual({
      createdFrom: null,
      createdTo: null,
      createdRelativeDateID: RelativeDate.WITHIN_1_WEEK,
      addedFrom: null,
      addedTo: null,
      addedRelativeDateID: RelativeDate.WITHIN_1_WEEK,
    })
  })

  it('setting a relative date clears absolute dates', () => {
    component.createdDateFrom = '2023-01-01'
    component.createdDateTo = '2023-02-01'
    component.createdRelativeDate = RelativeDate.TODAY // ngModel
    component.onSetCreatedRelativeDate()
    expect(component.createdDateFrom).toBeNull()
    expect(component.createdDateTo).toBeNull()
    expect(component.createdRelativeDate).toEqual(RelativeDate.TODAY)
  })

  it('should support report if active', () => {
    component.createdRelativeDate = RelativeDate.WITHIN_1_WEEK
    expect(component.isActive).toBeTruthy()
    component.createdRelativeDate = null
    component.createdDateFrom = '2023-05-30'
    expect(component.isActive).toBeTruthy()
    component.createdDateFrom = null
    component.createdDateTo = '2023-05-30'
    expect(component.isActive).toBeTruthy()
    component.createdDateTo = null

    component.addedRelativeDate = RelativeDate.WITHIN_1_WEEK
    expect(component.isActive).toBeTruthy()
    component.addedRelativeDate = null
    component.addedDateFrom = '2023-05-30'
    expect(component.isActive).toBeTruthy()
    component.addedDateFrom = null
    component.addedDateTo = '2023-05-30'
    expect(component.isActive).toBeTruthy()
    component.addedDateTo = null

    expect(component.isActive).toBeFalsy()
  })

  it('should support reset', () => {
    component.createdDateFrom = '2023-05-30'
    component.reset()
    expect(component.createdDateFrom).toBeNull()
  })

  it('should support clearFrom', () => {
    component.createdDateFrom = '2023-05-30'
    component.clearCreatedFrom()
    expect(component.createdDateFrom).toBeNull()

    component.addedDateFrom = '2023-05-30'
    component.clearAddedFrom()
    expect(component.addedDateFrom).toBeNull()
  })

  it('should support clearTo', () => {
    component.createdDateTo = '2023-05-30'
    component.clearCreatedTo()
    expect(component.createdDateTo).toBeNull()

    component.addedDateTo = '2023-05-30'
    component.clearAddedTo()
    expect(component.addedDateTo).toBeNull()
  })

  it('should support clearRelativeDate', () => {
    component.createdRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.clearCreatedRelativeDate()
    expect(component.createdRelativeDate).toBeNull()

    component.addedRelativeDate = RelativeDate.WITHIN_1_WEEK
    component.clearAddedRelativeDate()
    expect(component.addedRelativeDate).toBeNull()
  })

  it('should limit keyboard events', () => {
    expect(isAllowedDateInputKey('9')).toBeTruthy()
    expect(isAllowedDateInputKey('.')).toBeTruthy()
    expect(isAllowedDateInputKey('Enter')).toBeTruthy()
    expect(isAllowedDateInputKey('{')).toBeFalsy()
  })

  it('should support debounce', () => {
    vi.useFakeTimers()
    let result: DateSelection
    component.onChange((date) => (result = date))
    component.createdRelativeDate = RelativeDate.TODAY
    component.onChangeDebounce()
    vi.advanceTimersByTime(500)
    expect(result).toEqual({
      createdFrom: null,
      createdTo: null,
      createdRelativeDateID: null,
      addedFrom: null,
      addedTo: null,
      addedRelativeDateID: null,
    })
  })

  it('destroy cancels a pending debounce', () => {
    vi.useFakeTimers()
    let called = false
    component.onChange(() => (called = true))
    component.onChangeDebounce()
    component.destroy()
    vi.advanceTimersByTime(500)
    expect(called).toBeFalsy()
  })
})

describe('relative date options', () => {
  it('lists all options in Angular order with English labels', () => {
    const opts = getRelativeDates(new Date(2024, 4, 15, 10, 0, 0)) // Wed 15 May 2024
    expect(opts.map((o) => o.id)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
    expect(opts.map((o) => o.name)).toEqual(Object.values(RELATIVE_DATE_LABELS))
    expect(opts[0].name).toEqual('filter.within_1_week')
    const byId = (id: number) => opts.find((o) => o.id === id)
    expect(new Date(byId(RelativeDate.WITHIN_1_WEEK).dateTilNow).getDate()).toEqual(8)
    expect(byId(RelativeDate.THIS_YEAR).date).toEqual(new Date(2024, 0, 1))
    expect(byId(RelativeDate.THIS_YEAR).dateEnd).toEqual(new Date(2024, 11, 31))
    expect(byId(RelativeDate.THIS_MONTH).dateEnd).toEqual(new Date(2024, 4, 31))
    expect(byId(RelativeDate.PREVIOUS_WEEK).date).toEqual(new Date(2024, 4, 6))
    expect(byId(RelativeDate.PREVIOUS_WEEK).dateEnd).toEqual(new Date(2024, 4, 12))
    expect(byId(RelativeDate.PREVIOUS_MONTH).date).toEqual(new Date(2024, 3, 1))
    expect(byId(RelativeDate.PREVIOUS_MONTH).dateEnd).toEqual(new Date(2024, 3, 30))
    expect(byId(RelativeDate.PREVIOUS_QUARTER).date).toEqual(new Date(2024, 0, 1))
    expect(byId(RelativeDate.PREVIOUS_QUARTER).dateEnd).toEqual(new Date(2024, 2, 31))
    expect(byId(RelativeDate.PREVIOUS_YEAR).date).toEqual(new Date(2023, 0, 1))
    expect(todayISO(new Date(2024, 4, 15))).toEqual('2024-05-15')
  })
})
