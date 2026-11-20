// Ported from permissions-filter-dropdown.component.spec.ts
import { beforeEach, describe, expect, it } from 'vitest'
import { OwnerFilterType, PermissionsFilterState, PermissionsSelectionModel } from './permissions-model'

const currentUserID = 13

describe('PermissionsFilterState', () => {
  let component: PermissionsFilterState
  let ownerFilterSetResult: PermissionsSelectionModel

  beforeEach(() => {
    component = new PermissionsFilterState(new PermissionsSelectionModel(), {
      currentUserId: currentUserID,
      users: [
        { id: 1, username: 'user1' },
        { id: 10, username: 'user10' },
      ],
    })
    component.onChange((model) => (ownerFilterSetResult = model))
  })

  it('should report is active', () => {
    component.setFilter(OwnerFilterType.NONE)
    expect(component.isActive).toBeFalsy()
    component.setFilter(OwnerFilterType.OTHERS)
    expect(component.isActive).toBeTruthy()
    component.setFilter(OwnerFilterType.NONE)
    component.selectionModel.hideUnowned = true
    expect(component.isActive).toBeTruthy()
  })

  it('should describe concrete user filters honestly', () => {
    component.selectionModel.ownerFilter = OwnerFilterType.SELF
    component.selectionModel.userID = 1
    expect(component.ownerFilterLabel).toEqual('Owned by user1')

    component.selectionModel.ownerFilter = OwnerFilterType.NOT_SELF
    component.selectionModel.excludeUsers = [1]
    expect(component.ownerExclusionFilterLabel).toEqual('Not owned by user1')

    component.selectionModel.ownerFilter = OwnerFilterType.SHARED_BY_ME
    component.selectionModel.userID = 1
    expect(component.sharedByFilterLabel).toEqual('Shared by user1')
  })

  it('should describe concrete filters when usernames are unavailable', () => {
    component.selectionModel.ownerFilter = OwnerFilterType.SELF
    component.selectionModel.userID = 99
    expect(component.ownerFilterLabel).toEqual('Owned by someone else')

    component.selectionModel.ownerFilter = OwnerFilterType.NOT_SELF
    component.selectionModel.excludeUsers = [99]
    expect(component.ownerExclusionFilterLabel).toEqual('Not owned by someone else')

    component.selectionModel.excludeUsers = [98, 99]
    expect(component.ownerExclusionFilterLabel).toEqual('Not owned by the chosen users')

    component.selectionModel.ownerFilter = OwnerFilterType.SHARED_BY_ME
    component.selectionModel.userID = 99
    expect(component.sharedByFilterLabel).toEqual('Shared by someone else')
  })

  it('should retain relative labels for filters bound to the current user', () => {
    component.selectionModel.userID = currentUserID
    expect(component.ownerFilterLabel).toEqual('My documents')
    expect(component.sharedByFilterLabel).toEqual('Shared by me')

    component.selectionModel.excludeUsers = [currentUserID]
    expect(component.ownerExclusionFilterLabel).toEqual('Shared with me')
  })

  it('should retain relative labels for inactive filter choices', () => {
    component.selectionModel.ownerFilter = OwnerFilterType.NONE
    expect(component.ownerFilterLabel).toEqual('My documents')
    expect(component.ownerExclusionFilterLabel).toEqual('Shared with me')
    expect(component.sharedByFilterLabel).toEqual('Shared by me')
  })

  it('should translate labels with the given translator', () => {
    const c = new PermissionsFilterState(new PermissionsSelectionModel(), {
      currentUserId: currentUserID,
      users: [{ id: 1, username: 'user1' }],
      t: (k, p) => `[${k}|${p?.PH ?? ''}]`,
    })
    c.selectionModel.ownerFilter = OwnerFilterType.SELF
    c.selectionModel.userID = 1
    expect(c.ownerFilterLabel).toEqual('[filter.owned|user1]')
  })

  it('should support reset', () => {
    component.setFilter(OwnerFilterType.OTHERS)
    expect(component.selectionModel.ownerFilter).not.toEqual(OwnerFilterType.NONE)
    component.reset()
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.NONE)
  })

  it('should toggle owner filter type when users selected', () => {
    component.selectionModel.ownerFilter = OwnerFilterType.NONE

    // this would normally be done by select component
    component.selectionModel.includeUsers = [12]
    component.onUserSelect()
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.OTHERS)

    component.selectionModel.includeUsers = null
    component.onUserSelect()
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.NONE)

    component.setIncludeUsers([3])
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.OTHERS)
    expect(ownerFilterSetResult.includeUsers).toEqual([3])
  })

  it('should clear selected users', () => {
    component.selectionModel.includeUsers = [12]
    component.onUserSelect()
    component.selectionModel.hideUnowned = true
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.OTHERS)

    component.clearIncludeUsers()

    expect(component.selectionModel.includeUsers).toEqual([])
    expect(component.selectionModel.ownerFilter).toEqual(OwnerFilterType.NONE)
    expect(component.selectionModel.hideUnowned).toBeTruthy()
  })

  it('should emit a selection model depending on the type of owner filter set', () => {
    const emitted = () => ({
      excludeUsers: ownerFilterSetResult.excludeUsers,
      hideUnowned: ownerFilterSetResult.hideUnowned,
      includeUsers: ownerFilterSetResult.includeUsers,
      ownerFilter: ownerFilterSetResult.ownerFilter,
      userID: ownerFilterSetResult.userID,
    })
    component.selectionModel.ownerFilter = OwnerFilterType.NONE

    component.setFilter(OwnerFilterType.SELF)
    expect(emitted()).toEqual({
      excludeUsers: [],
      hideUnowned: false,
      includeUsers: [],
      ownerFilter: OwnerFilterType.SELF,
      userID: currentUserID,
    })

    component.setFilter(OwnerFilterType.NOT_SELF)
    expect(emitted()).toEqual({
      excludeUsers: [currentUserID],
      hideUnowned: false,
      includeUsers: [],
      ownerFilter: OwnerFilterType.NOT_SELF,
      userID: null,
    })

    component.setFilter(OwnerFilterType.NONE)
    expect(emitted()).toEqual({
      excludeUsers: [],
      hideUnowned: false,
      includeUsers: [],
      ownerFilter: OwnerFilterType.NONE,
      userID: null,
    })

    component.setFilter(OwnerFilterType.SHARED_BY_ME)
    expect(emitted()).toEqual({
      excludeUsers: [],
      hideUnowned: false,
      includeUsers: [],
      ownerFilter: OwnerFilterType.SHARED_BY_ME,
      userID: currentUserID,
    })

    component.setFilter(OwnerFilterType.UNOWNED)
    expect(emitted()).toEqual({
      excludeUsers: [],
      hideUnowned: false,
      includeUsers: [],
      ownerFilter: OwnerFilterType.UNOWNED,
      userID: null,
    })
  })

  it('should emit on hide unowned toggle', () => {
    let count = 0
    component.onChange(() => count++)
    component.setHideUnowned(true)
    expect(component.selectionModel.hideUnowned).toBeTruthy()
    expect(count).toEqual(1)
  })
})
