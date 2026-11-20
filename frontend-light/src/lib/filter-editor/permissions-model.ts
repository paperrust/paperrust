// Port of PermissionsSelectionModel / OwnerFilterType and the pure logic of
// PermissionsFilterDropdownComponent
// (src-ui/src/app/components/common/permissions-filter-dropdown/permissions-filter-dropdown.component.ts).
// Signals → plain fields; `ownerFilterSet` output → Emitter (`onChange(fn)`).

import { t as translate } from '../i18n'
import { Emitter, identityTranslator, type Translator } from './emitter'

export const OwnerFilterType = {
  NONE: 0,
  SELF: 1,
  NOT_SELF: 2,
  OTHERS: 3,
  UNOWNED: 4,
  SHARED_BY_ME: 5,
} as const
export type OwnerFilterType = (typeof OwnerFilterType)[keyof typeof OwnerFilterType]

export interface PermissionsUser {
  id: number
  username: string
  [key: string]: any
}

export class PermissionsSelectionModel {
  ownerFilter: OwnerFilterType = OwnerFilterType.NONE
  hideUnowned = false
  userID: number = null
  includeUsers: number[] = []
  excludeUsers: number[] = []

  clear(): void {
    this.ownerFilter = OwnerFilterType.NONE
    this.userID = null
    this.hideUnowned = false
    this.includeUsers = []
    this.excludeUsers = []
  }
}

/** i18n keys (English source) used by the permissions dropdown. */
export const PERMISSIONS_FILTER_KEYS = {
  myDocuments: 'filter.documents',
  ownedBy: 'filter.owned',
  ownedByAnother: 'filter.owned_another_user',
  sharedWithMe: 'filter.shared_3',
  notOwnedBy: 'filter.not_owned',
  notOwnedByAnother: 'filter.not_owned_another_user',
  notOwnedBySelected: 'filter.not_owned_selected_users',
  sharedByMe: 'filter.shared_2',
  sharedBy: 'filter.shared',
  sharedByAnother: 'filter.shared_another_user',
} as const

export interface PermissionsFilterOptions {
  /** Id of the logged-in user (Angular: `settingsService.currentUser().id`). */
  currentUserId?: number
  /** Users for label lookup (Angular loads them only with View/User permission). */
  users?: PermissionsUser[]
  t?: Translator
}

/** Framework-free state of `<pngx-permissions-filter-dropdown>`. */
export class PermissionsFilterState {
  /** Angular `ownerFilterSet` output. */
  readonly ownerFilterSet = new Emitter<PermissionsSelectionModel>()

  title: string = null
  disabled = false
  currentUserId: number
  users: PermissionsUser[]
  private readonly t: Translator

  constructor(
    public selectionModel: PermissionsSelectionModel = new PermissionsSelectionModel(),
    options: PermissionsFilterOptions = {}
  ) {
    this.currentUserId = options.currentUserId
    this.users = options.users ?? []
    this.t = options.t ?? translate
  }

  /** Subscribe to `ownerFilterSet`. Returns unsubscribe. */
  onChange(fn: (model: PermissionsSelectionModel) => void): () => void {
    return this.ownerFilterSet.subscribe(fn)
  }

  get isActive(): boolean {
    return this.selectionModel.ownerFilter !== OwnerFilterType.NONE || this.selectionModel.hideUnowned
  }

  get ownerFilterLabel(): string {
    if (
      this.selectionModel?.ownerFilter !== OwnerFilterType.SELF ||
      this.selectionModel?.userID === this.currentUserId
    ) {
      return this.t(PERMISSIONS_FILTER_KEYS.myDocuments)
    }

    const username = this.getUsername(this.selectionModel?.userID)
    return username
      ? this.t(PERMISSIONS_FILTER_KEYS.ownedBy, { PH: username })
      : this.t(PERMISSIONS_FILTER_KEYS.ownedByAnother)
  }

  get ownerExclusionFilterLabel(): string {
    const excludedUsers = this.selectionModel?.excludeUsers ?? []
    if (
      this.selectionModel?.ownerFilter !== OwnerFilterType.NOT_SELF ||
      (excludedUsers.length === 1 && excludedUsers[0] === this.currentUserId)
    ) {
      return this.t(PERMISSIONS_FILTER_KEYS.sharedWithMe)
    }

    const usernames = excludedUsers.map((id) => this.getUsername(id)).filter(Boolean)
    if (usernames.length === excludedUsers.length && usernames.length > 0) {
      return this.t(PERMISSIONS_FILTER_KEYS.notOwnedBy, { PH: usernames.join(', ') })
    }
    return excludedUsers.length === 1
      ? this.t(PERMISSIONS_FILTER_KEYS.notOwnedByAnother)
      : this.t(PERMISSIONS_FILTER_KEYS.notOwnedBySelected)
  }

  get sharedByFilterLabel(): string {
    if (
      this.selectionModel?.ownerFilter !== OwnerFilterType.SHARED_BY_ME ||
      this.selectionModel?.userID === this.currentUserId
    ) {
      return this.t(PERMISSIONS_FILTER_KEYS.sharedByMe)
    }

    const username = this.getUsername(this.selectionModel?.userID)
    return username
      ? this.t(PERMISSIONS_FILTER_KEYS.sharedBy, { PH: username })
      : this.t(PERMISSIONS_FILTER_KEYS.sharedByAnother)
  }

  reset(): void {
    this.selectionModel.clear()
    this.onChangeEmit()
  }

  setFilter(type: OwnerFilterType): void {
    const m = this.selectionModel
    m.ownerFilter = type
    if (m.ownerFilter === OwnerFilterType.SELF) {
      m.includeUsers = []
      m.excludeUsers = []
      m.userID = this.currentUserId
      m.hideUnowned = false
    } else if (m.ownerFilter === OwnerFilterType.NOT_SELF) {
      m.userID = null
      m.includeUsers = []
      m.excludeUsers = [this.currentUserId]
      m.hideUnowned = false
    } else if (m.ownerFilter === OwnerFilterType.NONE) {
      m.userID = null
      m.includeUsers = []
      m.excludeUsers = []
      m.hideUnowned = false
    } else if (m.ownerFilter === OwnerFilterType.SHARED_BY_ME) {
      m.userID = this.currentUserId
      m.includeUsers = []
      m.excludeUsers = []
      m.hideUnowned = false
    } else if (m.ownerFilter === OwnerFilterType.UNOWNED) {
      m.userID = null
      m.includeUsers = []
      m.excludeUsers = []
      m.hideUnowned = false
    }
    this.onChangeEmit()
  }

  /** Angular `onChange()` – emits `ownerFilterSet`. Call after changing `hideUnowned`. */
  onChangeEmit(): void {
    this.ownerFilterSet.next(this.selectionModel)
  }

  /** Hide-unowned checkbox: `(ngModelChange)` + `(change)="onChange()"`. */
  setHideUnowned(value: boolean): void {
    this.selectionModel.hideUnowned = value
    this.onChangeEmit()
  }

  /** User select: `(ngModelChange)="includeUsers.set($event)"` then `onUserSelect()`. */
  setIncludeUsers(users: number[]): void {
    this.selectionModel.includeUsers = users
    this.onUserSelect()
  }

  clearIncludeUsers(): void {
    this.selectionModel.includeUsers = []
    this.onUserSelect()
  }

  onUserSelect(): void {
    this.selectionModel.ownerFilter = this.selectionModel.includeUsers?.length
      ? OwnerFilterType.OTHERS
      : OwnerFilterType.NONE
    this.onChangeEmit()
  }

  private getUsername(userID: number): string {
    return this.users.find((user) => user.id === userID)?.username
  }
}
