// Ported 1:1 from src-ui/src/app/utils/query-params.ts.
// Angular's ParamMap / Params are replaced by a tiny local ParamMap interface
// (same semantics as @angular/router's convertToParamMap) and plain objects.
import {
  CustomFieldQueryLogicalOperator,
  CustomFieldQueryOperator,
} from './custom-field-query'
import {
  FILTER_CUSTOM_FIELDS_QUERY,
  FILTER_HAS_CUSTOM_FIELDS_ALL,
  FILTER_HAS_CUSTOM_FIELDS_ANY,
  FILTER_RULE_TYPES,
  FILTER_SIMPLE_TEXT,
  FILTER_SIMPLE_TITLE,
  FILTER_TITLE,
  FILTER_TITLE_CONTENT,
  NEGATIVE_NULL_FILTER_VALUE,
  SIMPLE_TEXT_PARAMETER,
  SIMPLE_TITLE_PARAMETER,
} from './filter-rule-type'
import type { FilterRuleType } from './filter-rule-type'
import type { FilterRule } from './filter-rules'

/** Equivalent of Angular's `Params`: a plain object of query parameters. */
export type Params = Record<string, any>

/** Equivalent of Angular's `ParamMap` (read-only view of query params). */
export interface ParamMap {
  has(name: string): boolean
  get(name: string): any
  getAll(name: string): any[]
  readonly keys: string[]
}

/** Accepted inputs wherever the Angular code took a ParamMap. */
export type ParamMapInput = ParamMap | Params | URLSearchParams | string

/** Minimal subset of the Angular ListViewState used here. */
export interface ListViewState {
  sortField: string
  sortReverse: boolean
  currentPage: number
  filterRules: FilterRule[]
}

/**
 * Adapter replacing Angular's `convertToParamMap`. Accepts a plain object
 * (values may be strings, numbers or arrays), a URLSearchParams instance, a
 * query string ("?a=1&b=2" / "a=1&b=2") or an existing ParamMap.
 */
export function convertToParamMap(params: ParamMapInput): ParamMap {
  if (typeof params === 'string') {
    params = new URLSearchParams(params)
  }
  if (params instanceof URLSearchParams) {
    const usp = params
    return {
      has: (name) => usp.has(name),
      get: (name) => usp.get(name),
      getAll: (name) => usp.getAll(name),
      get keys() {
        return Array.from(new Set(usp.keys()))
      },
    }
  }
  if (isParamMap(params)) return params
  const obj: Params = params ?? {}
  const own = (name: string) => Object.prototype.hasOwnProperty.call(obj, name)
  return {
    has: (name) => own(name),
    get: (name) => {
      if (!own(name)) return null
      const v = obj[name]
      return Array.isArray(v) ? v[0] : v
    },
    getAll: (name) => {
      if (!own(name)) return []
      const v = obj[name]
      return Array.isArray(v) ? v : [v]
    },
    get keys() {
      return Object.keys(obj)
    },
  }
}

function isParamMap(p: unknown): p is ParamMap {
  return (
    p != null &&
    typeof (p as ParamMap).has === 'function' &&
    typeof (p as ParamMap).get === 'function' &&
    typeof (p as ParamMap).getAll === 'function'
  )
}

/**
 * Serialises Params (as returned by paramsFromViewState /
 * queryParamsFromFilterRules) to URLSearchParams. Like Angular's router,
 * `undefined` / `null` values are dropped and arrays become repeated keys.
 */
export function paramsToURLSearchParams(params: Params): URLSearchParams {
  const usp = new URLSearchParams()
  if (!params) return usp
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue
    if (Array.isArray(value)) {
      for (const v of value) usp.append(key, String(v))
    } else {
      usp.append(key, String(value))
    }
  }
  return usp
}

const SORT_FIELD_PARAMETER = 'sort'
const SORT_REVERSE_PARAMETER = 'reverse'
const PAGE_PARAMETER = 'page'

export function paramsFromViewState(
  viewState: ListViewState,
  pageOnly: boolean = false
): Params {
  let params = queryParamsFromFilterRules(viewState.filterRules)
  params[SORT_FIELD_PARAMETER] = viewState.sortField
  params[SORT_REVERSE_PARAMETER] = viewState.sortReverse ? 1 : undefined
  if (pageOnly) params = {}
  params[PAGE_PARAMETER] = isNaN(viewState.currentPage)
    ? 1
    : viewState.currentPage
  if (pageOnly && viewState.currentPage == 1) params[PAGE_PARAMETER] = undefined
  return params
}

export function paramsToViewState(queryParams: ParamMapInput): ListViewState {
  const qp = convertToParamMap(queryParams)
  let filterRules = filterRulesFromQueryParams(qp)
  let sortField = qp.get(SORT_FIELD_PARAMETER)
  let sortReverse =
    qp.has(SORT_REVERSE_PARAMETER) ||
    (!qp.has(SORT_FIELD_PARAMETER) && !qp.has(SORT_REVERSE_PARAMETER))
  let currentPage = qp.has(PAGE_PARAMETER)
    ? parseInt(qp.get(PAGE_PARAMETER))
    : 1
  return {
    currentPage: currentPage,
    filterRules: filterRules,
    sortField: sortField,
    sortReverse: sortReverse,
  }
}

export function transformLegacyFilterRules(
  filterRules: FilterRule[]
): FilterRule[] {
  const LEGACY_CUSTOM_FIELD_FILTER_RULE_TYPES = [
    FILTER_HAS_CUSTOM_FIELDS_ANY,
    FILTER_HAS_CUSTOM_FIELDS_ALL,
  ]
  if (
    filterRules.filter((rule) =>
      LEGACY_CUSTOM_FIELD_FILTER_RULE_TYPES.includes(rule.rule_type)
    ).length
  ) {
    const anyRules = filterRules.filter(
      (rule) => rule.rule_type === FILTER_HAS_CUSTOM_FIELDS_ANY
    )
    const allRules = filterRules.filter(
      (rule) => rule.rule_type === FILTER_HAS_CUSTOM_FIELDS_ALL
    )
    const customFieldQueryLogicalOperator = allRules.length
      ? CustomFieldQueryLogicalOperator.And
      : CustomFieldQueryLogicalOperator.Or
    const valueRules = allRules.length ? allRules : anyRules
    const customFieldQueryExpression = [
      customFieldQueryLogicalOperator,
      [
        ...valueRules.map((rule) => [
          parseInt(rule.value),
          CustomFieldQueryOperator.Exists,
          true,
        ]),
      ],
    ]
    filterRules.push({
      rule_type: FILTER_CUSTOM_FIELDS_QUERY,
      value: JSON.stringify(customFieldQueryExpression),
    })
  }
  // TODO: can we support FILTER_DOES_NOT_HAVE_CUSTOM_FIELDS or FILTER_HAS_ANY_CUSTOM_FIELDS?
  return filterRules.filter(
    (rule) => !LEGACY_CUSTOM_FIELD_FILTER_RULE_TYPES.includes(rule.rule_type)
  )
}

export function filterRulesFromQueryParams(
  queryParams: ParamMapInput
): FilterRule[] {
  const qp = convertToParamMap(queryParams)
  let filterRulesFromQueryParams: FilterRule[] = []

  const allFilterRuleQueryParams: string[] = FILTER_RULE_TYPES.map(
    (rt) => rt.filtervar
  )
    .concat(FILTER_RULE_TYPES.map((rt) => rt.isnull_filtervar))
    .filter((rt) => rt !== undefined)

  // transform query params to filter rules
  allFilterRuleQueryParams
    .filter((frqp) => qp.has(frqp))
    .forEach((filterQueryParamName) => {
      const rule_type: FilterRuleType = FILTER_RULE_TYPES.find(
        (rt) =>
          rt.filtervar == filterQueryParamName ||
          rt.isnull_filtervar == filterQueryParamName
      )
      const isNullRuleType = rule_type.isnull_filtervar == filterQueryParamName
      const nullRuleValue =
        qp.get(filterQueryParamName) == '1'
          ? null
          : NEGATIVE_NULL_FILTER_VALUE.toString()
      const valueURIComponent: string = qp.get(filterQueryParamName)
      const filterQueryParamValues: string[] = rule_type.multi
        ? valueURIComponent.split(',')
        : [valueURIComponent]

      filterRulesFromQueryParams = filterRulesFromQueryParams.concat(
        // map all values to filter rules
        filterQueryParamValues.map((val) => {
          if (rule_type.datatype == 'boolean')
            val = val.replace('1', 'true').replace('0', 'false')
          return {
            rule_type: rule_type.id,
            value: isNullRuleType ? nullRuleValue : val,
          }
        })
      )
    })
  filterRulesFromQueryParams = transformLegacyFilterRules(
    filterRulesFromQueryParams
  )
  return filterRulesFromQueryParams
}

export function queryParamsFromFilterRules(filterRules: FilterRule[]): Params {
  if (filterRules) {
    let params: Params = {}
    for (let rule of filterRules) {
      let ruleType = FILTER_RULE_TYPES.find((t) => t.id == rule.rule_type)
      if (
        rule.rule_type === FILTER_TITLE_CONTENT ||
        rule.rule_type === FILTER_SIMPLE_TEXT
      ) {
        params[SIMPLE_TEXT_PARAMETER] = rule.value
      } else if (
        rule.rule_type === FILTER_TITLE ||
        rule.rule_type === FILTER_SIMPLE_TITLE
      ) {
        params[SIMPLE_TITLE_PARAMETER] = rule.value
      } else if (ruleType.isnull_filtervar && rule.value == null) {
        params[ruleType.isnull_filtervar] = 1
      } else if (
        ruleType.isnull_filtervar &&
        rule.value == NEGATIVE_NULL_FILTER_VALUE.toString()
      ) {
        params[ruleType.isnull_filtervar] = 0
      } else if (ruleType.multi) {
        params[ruleType.filtervar] = params[ruleType.filtervar]
          ? params[ruleType.filtervar] + ',' + rule.value
          : rule.value
      } else {
        params[ruleType.filtervar] = rule.value
        if (ruleType.datatype == 'boolean')
          params[ruleType.filtervar] =
            rule.value == 'true' || rule.value == '1' ? 1 : 0
      }
    }
    return params
  } else {
    return null
  }
}
