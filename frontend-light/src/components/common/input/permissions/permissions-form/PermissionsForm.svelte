<!-- Port of common/input/permissions/permissions-form (with accordion variant like NgbAccordion, content only on expanding – destroyOnHide). Host <pngx-permissions-form>
     in the parent markup. Value: { owner, set_permissions: { view: {users, groups}, change: {users, groups} } }. -->
<script lang="ts" module>
  const accSeq = { n: 0 }
</script>

<script lang="ts">
  import '../../../../../styles/components/permissions-form.css'
  import { t } from '../../../../../lib/i18n'
  import type { User } from '../../../../../api/v1'
  import InputSelect from '../../select/InputSelect.svelte'
  import PermissionsUser from '../permissions-user/PermissionsUser.svelte'
  import PermissionsGroup from '../permissions-group/PermissionsGroup.svelte'

  interface PermissionsFormObject {
    owner: number | null
    set_permissions: { view: { users: number[]; groups: number[] }; change: { users: number[]; groups: number[] } }
  }
  let {
    value = $bindable(),
    users = [],
    title = t('common.permissions'),
    disabled = false,
    accordion = false,
    onchange,
  }: { value?: PermissionsFormObject; users?: User[]; title?: string; disabled?: boolean; accordion?: boolean; onchange?: (v: PermissionsFormObject) => void } = $props()
  // NgbAccordion: element IDs ngb-accordion-item-<n>, collapsed (activeIds="")
  const accId = `ngb-accordion-item-${accSeq.n++}`
  let accOpen = $state(false)

  const empty = { users: [] as number[], groups: [] as number[] }
  const sp = $derived(value?.set_permissions ?? { view: empty, change: empty })
  function patch(fn: (v: PermissionsFormObject) => void) {
    const v: PermissionsFormObject = JSON.parse(JSON.stringify({ owner: value?.owner ?? null, set_permissions: sp }))
    fn(v)
    value = v
    onchange?.(v)
  }
</script>

{#snippet permissionsForm()}
<div>
  <div class="row">
    <div class="col-lg-3">
      <label class="form-label d-block my-2">{t('input.owner_label')}</label>
    </div>
    <div class="col-lg-9">
      <pngx-input-select><InputSelect items={users} bindLabel="username" value={value?.owner ?? null} allowNull={true} {disabled} onchange={(o) => patch((v) => (v.owner = o))} /></pngx-input-select>
    </div>
  </div>
  <small class="form-text text-muted text-end d-block mt-n2">{t('common.objects_without_owner_viewed')}</small>
  <div>
    <h6 class="mt-3">{t('common.view')}</h6>
    <div class="mb-2">
      <div class="row mb-1">
        <div class="col-lg-3">
          <label class="form-label d-block my-2">{t('common.users_label')}</label>
        </div>
        <div class="col-lg-9">
          <pngx-permissions-user type="view"><PermissionsUser value={sp.view.users} {disabled} onchange={(u) => patch((v) => (v.set_permissions.view.users = u))} /></pngx-permissions-user>
        </div>
      </div>
      <div class="row">
        <div class="col-lg-3">
          <label class="form-label d-block my-2">{t('common.groups_label')}</label>
        </div>
        <div class="col-lg-9">
          <pngx-permissions-group type="view"><PermissionsGroup value={sp.view.groups} {disabled} onchange={(g) => patch((v) => (v.set_permissions.view.groups = g))} /></pngx-permissions-group>
        </div>
      </div>
    </div>
    <h6 class="mt-4">{t('common.edit')}</h6>
    <div>
      <div class="row mb-1">
        <div class="col-lg-3">
          <label class="form-label d-block my-2">{t('common.users_label')}</label>
        </div>
        <div class="col-lg-9">
          <pngx-permissions-user type="change"><PermissionsUser value={sp.change.users} {disabled} onchange={(u) => patch((v) => (v.set_permissions.change.users = u))} /></pngx-permissions-user>
        </div>
      </div>
      <div class="row">
        <div class="col-lg-3">
          <label class="form-label d-block my-2">{t('common.groups_label')}</label>
        </div>
        <div class="col-lg-9">
          <pngx-permissions-group type="change"><PermissionsGroup value={sp.change.groups} {disabled} onchange={(g) => patch((v) => (v.set_permissions.change.groups = g))} /></pngx-permissions-group>
        </div>
      </div>
      <small class="form-text text-muted text-end d-block">{t('common.edit_permissions_also_grant')}</small>
    </div>
  </div>
</div>
{/snippet}

{#if !accordion}
  {#if title?.length > 0}
    <h5>{title}</h5>
  {/if}
  {@render permissionsForm()}
{/if}
{#if accordion}
  <div class="accordion">
    <div class="accordion-item">
      <h2 role="heading" class="accordion-header">
        <button type="button" class="accordion-button" class:collapsed={!accOpen} id="{accId}-toggle" aria-controls="{accId}-collapse" aria-expanded={accOpen} onclick={() => (accOpen = !accOpen)}>{t('common.edit_permissions')}</button>
      </h2>
      <div role="region" class="accordion-collapse collapse" class:show={accOpen} id="{accId}-collapse" aria-labelledby="{accId}-toggle">
        <div class="accordion-body">
          {#if accOpen}
            {@render permissionsForm()}
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}
