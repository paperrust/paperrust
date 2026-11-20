// Ported from src-ui/src/app/data/custom-field-query.ts (+ the CustomFieldDataType
// enum from data/custom-field.ts). Angular enums are expressed as `as const`
// objects with identical string values. Operator labels are plain English
// strings here (the Angular original wraps them in $localize); translate at the
// call site with the light UI's i18n helper if needed.

export const CustomFieldDataType = {
  String: 'string',
  Url: 'url',
  Date: 'date',
  Boolean: 'boolean',
  Integer: 'integer',
  Float: 'float',
  Monetary: 'monetary',
  DocumentLink: 'documentlink',
  Select: 'select',
  LongText: 'longtext',
} as const
export type CustomFieldDataType =
  (typeof CustomFieldDataType)[keyof typeof CustomFieldDataType]

export const CustomFieldQueryLogicalOperator = {
  And: 'AND',
  Or: 'OR',
  Not: 'NOT',
} as const
export type CustomFieldQueryLogicalOperator =
  (typeof CustomFieldQueryLogicalOperator)[keyof typeof CustomFieldQueryLogicalOperator]

export const CustomFieldQueryOperator = {
  Exact: 'exact',
  In: 'in',
  IsNull: 'isnull',
  Exists: 'exists',
  Contains: 'contains',
  IContains: 'icontains',
  GreaterThan: 'gt',
  GreaterThanOrEqual: 'gte',
  LessThan: 'lt',
  LessThanOrEqual: 'lte',
  Range: 'range',
} as const
export type CustomFieldQueryOperator =
  (typeof CustomFieldQueryOperator)[keyof typeof CustomFieldQueryOperator]

export const CUSTOM_FIELD_QUERY_OPERATOR_LABELS: Record<CustomFieldQueryOperator, string> = {
  [CustomFieldQueryOperator.Exact]: 'filter.equal',
  [CustomFieldQueryOperator.In]: 'filter.in',
  [CustomFieldQueryOperator.IsNull]: 'filter.null',
  [CustomFieldQueryOperator.Exists]: 'filter.exists',
  [CustomFieldQueryOperator.Contains]: 'filter.contains',
  [CustomFieldQueryOperator.IContains]: 'filter.contains_case_insensitive',
  [CustomFieldQueryOperator.GreaterThan]: 'filter.greater_2',
  [CustomFieldQueryOperator.GreaterThanOrEqual]: 'filter.greater_equal',
  [CustomFieldQueryOperator.LessThan]: 'filter.less_2',
  [CustomFieldQueryOperator.LessThanOrEqual]: 'filter.less_equal',
  [CustomFieldQueryOperator.Range]: 'common.range',
}

export const CustomFieldQueryOperatorGroups = {
  Basic: 'basic',
  Exact: 'exact',
  String: 'string',
  Arithmetic: 'arithmetic',
  Containment: 'containment',
  Subset: 'subset',
  Date: 'date',
} as const
export type CustomFieldQueryOperatorGroups =
  (typeof CustomFieldQueryOperatorGroups)[keyof typeof CustomFieldQueryOperatorGroups]

// Modified from filters.py > SUPPORTED_EXPR_OPERATORS
export const CUSTOM_FIELD_QUERY_OPERATORS_BY_GROUP = {
  [CustomFieldQueryOperatorGroups.Basic]: [
    CustomFieldQueryOperator.Exists,
    CustomFieldQueryOperator.IsNull,
  ],
  [CustomFieldQueryOperatorGroups.Exact]: [CustomFieldQueryOperator.Exact],
  [CustomFieldQueryOperatorGroups.String]: [CustomFieldQueryOperator.IContains],
  [CustomFieldQueryOperatorGroups.Arithmetic]: [
    CustomFieldQueryOperator.GreaterThan,
    CustomFieldQueryOperator.GreaterThanOrEqual,
    CustomFieldQueryOperator.LessThan,
    CustomFieldQueryOperator.LessThanOrEqual,
  ],
  [CustomFieldQueryOperatorGroups.Containment]: [
    CustomFieldQueryOperator.Contains,
  ],
  [CustomFieldQueryOperatorGroups.Subset]: [CustomFieldQueryOperator.In],
  [CustomFieldQueryOperatorGroups.Date]: [
    CustomFieldQueryOperator.GreaterThanOrEqual,
    CustomFieldQueryOperator.LessThanOrEqual,
  ],
}

// filters.py > SUPPORTED_EXPR_CATEGORIES
export const CUSTOM_FIELD_QUERY_OPERATOR_GROUPS_BY_TYPE = {
  [CustomFieldDataType.String]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.String,
  ],
  [CustomFieldDataType.Url]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.String,
  ],
  [CustomFieldDataType.Date]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.Date,
  ],
  [CustomFieldDataType.Boolean]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
  ],
  [CustomFieldDataType.Integer]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.Arithmetic,
  ],
  [CustomFieldDataType.Float]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.Arithmetic,
  ],
  [CustomFieldDataType.Monetary]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.String,
    CustomFieldQueryOperatorGroups.Arithmetic,
  ],
  [CustomFieldDataType.DocumentLink]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Containment,
  ],
  [CustomFieldDataType.Select]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.Exact,
    CustomFieldQueryOperatorGroups.Subset,
  ],
  [CustomFieldDataType.LongText]: [
    CustomFieldQueryOperatorGroups.Basic,
    CustomFieldQueryOperatorGroups.String,
  ],
}

export const CUSTOM_FIELD_QUERY_VALUE_TYPES_BY_OPERATOR = {
  [CustomFieldQueryOperator.Exact]: 'string|boolean',
  [CustomFieldQueryOperator.IsNull]: 'boolean',
  [CustomFieldQueryOperator.Exists]: 'boolean',
  [CustomFieldQueryOperator.IContains]: 'string',
  [CustomFieldQueryOperator.GreaterThanOrEqual]: 'string|number',
  [CustomFieldQueryOperator.LessThanOrEqual]: 'string|number',
  [CustomFieldQueryOperator.GreaterThan]: 'number',
  [CustomFieldQueryOperator.LessThan]: 'number',
  [CustomFieldQueryOperator.Contains]: 'array',
  [CustomFieldQueryOperator.In]: 'array',
}

export const CUSTOM_FIELD_QUERY_MAX_DEPTH = 4
export const CUSTOM_FIELD_QUERY_MAX_ATOMS = 5

export const CustomFieldQueryElementType = {
  Atom: 'Atom',
  Expression: 'Expression',
} as const
export type CustomFieldQueryElementType =
  (typeof CustomFieldQueryElementType)[keyof typeof CustomFieldQueryElementType]
