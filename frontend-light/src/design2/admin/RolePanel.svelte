<!-- View/edit/create role (custom mode): name, description, permissions by topic.
     Fixed roles and administrator view only (copying possible). API: POST/PATCH/DELETE /rs/api/v2/roles. -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import Icon from '../components/Icon.svelte'
  import Panel from './Panel.svelte'
  import TopicSwitches from './TopicSwitches.svelte'
  import { createRole, deleteRole, RolesError, updateRole } from '../api/roles'
  import { applyModelSwitch, applySwitch, qualify, rolePerms, summarize, withoutAlways, type Action, type Role, type RolesInfo } from './roles'
  import { roleDesc, roleName, summarySentence } from './labels'

  let {
    role,
    copyOf = null,
    info,
    startDelete = false,
    onclose,
    onsaved,
    ondeleted,
    oncopy,
  }: {
    role: Role | null
    copyOf?: Role | null
    info: RolesInfo
    startDelete?: boolean
    onclose: () => void
    onsaved: (msg: string) => void
    ondeleted: (msg: string) => void
    oncopy: (r: Role) => void
  } = $props()

  let panel: Panel
  export function requestClose(then?: () => void) {
    panel.requestClose(then)
  }

  // Panel is recreated per selection ({#key}): take over props once here
  const S = (() => {
    const isNew = !role
    const editable = info.mode === 'custom' && !info.fallback && (isNew || (!role!.builtin && !role!.admin))
    const src = role ?? copyOf
    const start = {
      name: role ? roleName(role) : copyOf ? t('m.admin.copy_of', { PH: roleName(copyOf) }) : '',
      description: role ? roleDesc(role) : copyOf ? roleDesc(copyOf) : '',
      perms: src && !src.admin ? withoutAlways(rolePerms(src), info.always) : [],
    }
    return { isNew, editable, start, confirmDelete: startDelete && editable && !isNew, title: isNew ? t('m.admin.new_role') : roleName(role!) }
  })()
  const { isNew, editable, start, title } = S
  let form = $state(JSON.parse(JSON.stringify(start)) as typeof start)
  const dirty = $derived(editable && JSON.stringify(form) !== JSON.stringify(start))
  let busy = $state(false)
  let error = $state<string | null>(null)
  let confirmDelete = $state(S.confirmDelete)
  const toggle = (topic: string, action: Action, on: boolean) => (form.perms = applySwitch(form.perms, info.topics, topic, action, on))
  const toggleModel = (model: string, action: Action, on: boolean) => (form.perms = applyModelSwitch(form.perms, model, action, on))

  async function save() {
    error = null
    if (!form.name.trim()) {
      error = t('m.admin.name_required')
      return
    }
    busy = true
    try {
      // only code names of the topic models; `always` is added by the API itself on assigning
      const permissions = qualify(withoutAlways(form.perms, info.always), info)
      const body = { name: form.name.trim(), description: form.description.trim(), permissions }
      if (isNew) await createRole(body)
      else await updateRole(role!.id, body)
      onsaved(t('m.admin.role_saved', { PH: body.name }))
    } catch (e) {
      error = e instanceof RolesError ? e.message : t('m.admin.error_save')
    } finally {
      busy = false
    }
  }
  async function doDelete() {
    busy = true
    error = null
    try {
      await deleteRole(role!.id)
      ondeleted(t('m.admin.role_deleted', { PH: roleName(role!) }))
    } catch (e) {
      error = e instanceof RolesError ? e.message : t('m.admin.error_delete')
      confirmDelete = false
    } finally {
      busy = false
    }
  }
</script>

<Panel bind:this={panel} {title} {dirty} {onclose}>
  <form id="ad-role-form" onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off" novalidate>
    {#if role?.admin}<p class="ad-note ad-flush">{t('m.admin.role_admin_desc')}</p>
    {:else if !editable && role}<p class="ad-note ad-flush">{t('m.admin.role_fixed_hint')}</p>{/if}
    <fieldset disabled={!editable || busy} class="ad-fs">
      <div class="ad-field"><label for="ad-r-name">{t('common.name')}</label><input id="ad-r-name" class="d2-input" bind:value={form.name} required data-autofocus={isNew || undefined} /></div>
      <div class="ad-field"><label for="ad-r-desc">{t('m.admin.description')}</label><textarea id="ad-r-desc" class="d2-textarea" rows="2" bind:value={form.description}></textarea></div>
    </fieldset>
    {#if !role?.admin}
      <section class="ad-sec">
        <h3>{t('m.admin.rights')}</h3>
        <TopicSwitches topics={info.topics} perms={form.perms} onchange={toggle} onmodel={toggleModel} disabled={!editable || busy} idPrefix="ad-r-sw" />
        <div class="ad-eff" aria-live="polite"><strong>{t('m.admin.effective')}</strong>{summarySentence(summarize(form.perms, info.topics))}</div>
      </section>
    {/if}
  </form>

  {#snippet confirm()}
    {#if confirmDelete}
      <div class="ad-confirm danger" role="group" aria-labelledby="ad-r-del-q">
        <p id="ad-r-del-q">{t('m.admin.delete_role_q', { PH: roleName(role!) })}</p>
        <div class="ad-line">
          <button type="button" class="d2-btn" onclick={() => (confirmDelete = false)} disabled={busy}>{t('common.cancel')}</button>
          <button type="button" class="d2-btn ad-danger-solid" onclick={doDelete} disabled={busy}>{t('common.delete')}</button>
        </div>
      </div>
    {/if}
  {/snippet}

  {#snippet footer()}
    {#if error}<p class="ad-err ad-full" role="alert">{error}</p>{/if}
    {#if editable && !isNew}
      <button type="button" class="d2-btn ghost danger" onclick={() => (confirmDelete = true)} disabled={busy}><Icon name="trash" />{t('common.delete')}</button>
    {/if}
    {#if role && !role.admin && info.mode === 'custom' && !info.fallback}
      <button type="button" class="d2-btn ghost" onclick={() => panel.requestClose(() => oncopy(role!))} disabled={busy}>{t('m.admin.copy_role')}</button>
    {/if}
    <span class="ad-grow"></span>
    <button type="button" class="d2-btn" onclick={() => panel.requestClose()} disabled={busy}>{editable ? t('common.cancel') : t('common.close')}</button>
    {#if editable}<button type="submit" form="ad-role-form" class="d2-btn primary" disabled={busy || (!dirty && !isNew)}>{t('common.save')}</button>{/if}
  {/snippet}
</Panel>

<style>
  .ad-fs {
    margin: 0;
    padding: 0;
    border: 0;
    min-width: 0;
  }
</style>
