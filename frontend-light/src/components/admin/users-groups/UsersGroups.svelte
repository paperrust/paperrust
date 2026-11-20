<!-- Port of admin/users-groups/users-groups.component.html (route usersgroups). Host <pngx-users-groups> in App.svelte. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { modals } from '../../../lib/modal.svelte'
  import { session } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { deleteOf, listAllOf } from '../../../api/admin'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import UserEditDialog from '../../common/edit-dialog/user-edit-dialog/UserEditDialog.svelte'
  import GroupEditDialog from '../../common/edit-dialog/group-edit-dialog/GroupEditDialog.svelte'

  let users = $state<any[] | undefined>(undefined)
  let groups = $state<any[] | undefined>(undefined)
  const canViewUsers = session.can('view', 'user')
  const canViewGroups = session.can('view', 'group')

  if (canViewUsers)
    listAllOf<any>('users', { full_perms: true })
      .then((r) => (users = r.results))
      .catch((e) => toastService.showError(t('admin.users.error_retrieving_users'), e))
  if (canViewGroups)
    listAllOf<any>('groups', { full_perms: true })
      .then((r) => (groups = r.results))
      .catch((e) => toastService.showError(t('common.error_retrieving_groups'), e))

  const reloadUsers = () => listAllOf<any>('users').then((r) => (users = r.results))
  const reloadGroups = () => listAllOf<any>('groups').then((r) => (groups = r.results))
  const getGroupName = (id: number) => groups?.find((g) => g.id === id)?.name ?? ''

  function editUser(user: any = null) {
    let passwordSet = false
    modals.open(
      UserEditDialog,
      {
        dialogMode: user ? 'edit' : 'create',
        object: user,
        onpasswordset: (v: boolean) => (passwordSet = v),
        onsucceeded: (newUser: any) => {
          if (newUser.id === session.data?.user?.id && passwordSet) {
            toastService.showInfo(t('admin.users.password_changed_logged_out'))
            setTimeout(() => {
              const basePath = '/'
              location.href = `${location.origin}${basePath}accounts/logout/?next=${basePath}accounts/login/?next=${basePath}`
            }, 2500)
          } else {
            toastService.showSuccess(t('admin.users.saved_user', { PH: newUser.username }))
            reloadUsers()
          }
        },
        onfailed: (e: any) => toastService.showError(t('admin.users.error_saving_user'), e),
      },
      { backdrop: 'static', size: 'xl' },
    )
  }

  function deleteUser(user: any) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('admin.users.confirm_delete_user_account'),
        messageBold: t('admin.users.operation_permanently_delete_user'),
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await deleteOf('users', user.id)
            ref.close()
            toastService.showSuccess(t('admin.users.deleted_user', { PH: user.username }))
            reloadUsers()
          } catch (e) {
            toastService.showError(t('admin.users.error_deleting_user', { PH: user.username }), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  function editGroup(group: any = null) {
    modals.open(
      GroupEditDialog,
      {
        dialogMode: group ? 'edit' : 'create',
        object: group,
        onsucceeded: (newGroup: any) => {
          toastService.showSuccess(t('admin.users.saved_group', { PH: newGroup.name }))
          reloadGroups()
        },
        onfailed: (e: any) => toastService.showError(t('admin.users.error_saving_group'), e),
      },
      { backdrop: 'static', size: 'lg' },
    )
  }

  function deleteGroup(group: any) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title: t('admin.users.confirm_delete_user_group'),
        messageBold: t('admin.users.operation_permanently_delete_user_2'),
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await deleteOf('groups', group.id)
            ref.close()
            toastService.showSuccess(t('admin.users.deleted_group', { PH: group.name }))
            reloadGroups()
          } catch (e) {
            toastService.showError(t('admin.users.error_deleting_group', { PH: group.name }), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }
</script>

<pngx-page-header>
<PageHeader
  title={t('common.users_groups')}
  info={t('admin.users.create_delete_edit_users')}
  infoLink="usage/#users-and-groups"
  >
</PageHeader>
</pngx-page-header>

{#if canViewUsers && users}
  <h4 class="d-flex">
    {t('common.users')}
    {#if session.can('add', 'user')}
      <button type="button" class="btn btn-sm btn-outline-primary ms-4" onclick={() => editUser()}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('admin.users.add_user')}
      </button>
    {/if}
  </h4>
  <ul class="list-group">
    <li class="list-group-item">
      <div class="row">
        <div class="col">{t('common.username')}</div>
        <div class="col">{t('common.name')}</div>
        <div class="col">{t('common.groups')}</div>
        <div class="col">{t('common.actions')}</div>
      </div>
    </li>
    {#each users as user (user)}
      <li class="list-group-item">
        <div class="row">
          <div class="col d-flex align-items-center" class:opacity-50={!user.is_active}><button class="btn btn-link p-0 text-start" type="button" onclick={() => editUser(user)} disabled={!session.can('change', 'user')}>{user.username}</button></div>
          <div class="col d-flex align-items-center">{user.first_name} {user.last_name}</div>
          <div class="col d-flex align-items-center">{user.groups?.map(getGroupName).join(', ')}</div>
          <div class="col">
            <div class="btn-group">
              {#if session.can('change', 'user')}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editUser(user)}>
                  <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                </button>
              {/if}
              {#if session.can('delete', 'user')}
                <button class="btn btn-sm btn-outline-danger" type="button" onclick={() => deleteUser(user)}>
                  <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                </button>
              {/if}
            </div>
          </div>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if canViewGroups && groups}
  <h4 class="mt-4 d-flex">
    {t('common.groups')}
    {#if session.can('add', 'group')}
      <button type="button" class="btn btn-sm btn-outline-primary ms-4" onclick={() => editGroup()}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('admin.users.add_group')}
      </button>
    {/if}
  </h4>
  <ul class="list-group">
    <li class="list-group-item">
      <div class="row">
        <div class="col">{t('common.name')}</div>
        <div class="col"></div>
        <div class="col"></div>
        <div class="col">{t('common.actions')}</div>
      </div>
    </li>
    {#each groups as group (group)}
      <li class="list-group-item">
        <div class="row">
          <div class="col d-flex align-items-center"><button class="btn btn-link p-0 text-start" type="button" onclick={() => editGroup(group)} disabled={!session.can('change', 'group')}>{group.name}</button></div>
          <div class="col"></div>
          <div class="col"></div>
          <div class="col">
            <div class="btn-group">
              {#if session.can('change', 'group')}
                <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editGroup(group)}>
                  <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                </button>
              {/if}
              {#if session.can('delete', 'group')}
                <button class="btn btn-sm btn-outline-danger" type="button" onclick={() => deleteGroup(group)}>
                  <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                </button>
              {/if}
            </div>
          </div>
        </div>
      </li>
    {/each}
    {#if groups.length === 0}
      <li class="list-group-item">{t('admin.users.no_groups_defined')}</li>
    {/if}
  </ul>
{/if}

{#if (canViewUsers && !users) || (canViewGroups && !groups)}
  <div>
    <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
    <div class="visually-hidden">{t('common.loading')}</div>
  </div>
{/if}
