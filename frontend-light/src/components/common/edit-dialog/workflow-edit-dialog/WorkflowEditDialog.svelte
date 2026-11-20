<!-- Port of common/edit-dialog/workflow-edit-dialog/workflow-edit-dialog.component.html (size xl): triggers and
     actions in NgbAccordion (collapsed, content only on expanding), actions sortable by dragging (cdkDrag → HTML5). -->
<script lang="ts" module>
  let accSeq = 0
</script>

<script lang="ts">
  import '../../../../styles/components/workflow-edit-dialog.css'
  import { t, tRich } from '../../../../lib/i18n'
  import { icon } from '../../bootstrap-icon/icon'
  import { ricon } from '../../../../lib/route-icons'
  import { listAllOf } from '../../../../api/admin'
  import { matchingAlgorithms } from '../../../../lib/matching'
  import { CustomFieldQueriesModel, CustomFieldQueryExpression, CustomFieldsQueryDropdownState } from '../../../../lib/filter-editor/custom-field-queries'
  import { data } from '../../../../stores/data.svelte'
  import { rawValue, session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import { toastService } from '../../../../stores/toasts.svelte'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputNumber from '../../input/number/InputNumber.svelte'
  import InputSwitch from '../../input/switch/InputSwitch.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import InputCheck from '../../input/check/InputCheck.svelte'
  import InputTags from '../../input/tags/InputTags.svelte'
  import InputTextarea from '../../input/textarea/InputTextarea.svelte'
  import InputEntries from '../../input/entries/InputEntries.svelte'
  import CustomFieldsValues from '../../input/custom-fields-values/CustomFieldsValues.svelte'
  import CustomFieldsQueryDropdown from '../../custom-fields-query-dropdown/CustomFieldsQueryDropdown.svelte'
  import PermissionsUser from '../../input/permissions/permissions-user/PermissionsUser.svelte'
  import PermissionsGroup from '../../input/permissions/permissions-group/PermissionsGroup.svelte'
  import ConfirmButton from '../../confirm-button/ConfirmButton.svelte'
  import './workflow-icons'
  import {
    AI_SUGGESTION_FIELD_OPTIONS,
    DOCUMENT_SOURCE_OPTIONS,
    MATCH_AUTO,
    MATCH_NONE,
    SCHEDULE_DATE_FIELD_OPTIONS,
    WORKFLOW_TYPE_OPTIONS,
    WorkflowActionType,
    WorkflowTriggerType,
    triggerHasDocumentFilters,
    actionForm,
    allowedActionTypes,
    defaultFilterValue,
    filterTypeOptions,
    getFilterDefinition,
    isCustomFieldQueryFilter,
    isRemovalDisabled,
    isSelectMultiple,
    isTagsFilter,
    moveItem,
    newAction,
    newTrigger,
    nextFilterType,
    triggerForm,
    workflowFormValues,
    type FilterForm,
    type TriggerFilterType,
  } from './workflow-edit'

  let props: EditDialogProps<any> = $props()
  // order: default 0 like the model (documents.Workflow.order default=0); upstream form sends null → 400 (A-221)
  const dlg = new EditDialogState<any>('workflows' as any, props, { name: null, order: 0, enabled: true, triggers: [], actions: [] })
  dlg.form.triggers = (dlg.object?.triggers ?? []).map(triggerForm)
  dlg.form.actions = (dlg.object?.actions ?? []).map(actionForm)
  const title = $derived(dlg.mode === 'create' ? t('dialog.workflow.create_new_workflow') : t('dialog.workflow.edit_workflow'))

  let relatedErrorShown = false
  const load = (ep: string, set: (r: any[]) => void) =>
    listAllOf<any>(ep)
      .then((r) => set(r.results))
      .catch((e) => {
        if (!relatedErrorShown) {
          relatedErrorShown = true
          toastService.showError(t('dialog.workflow.some_workflow_options_could'), e)
        }
        set([])
      })
  let correspondents = $state<any[] | undefined>(undefined)
  let documentTypes = $state<any[] | undefined>(undefined)
  let storagePaths = $state<any[] | undefined>(undefined)
  let mailRules = $state<any[] | undefined>(undefined)
  let customFields = $state<any[] | undefined>(undefined)
  load('correspondents', (r) => (correspondents = r))
  load('document_types', (r) => (documentTypes = r))
  load('storage_paths', (r) => (storagePaths = r))
  load('mail_rules', (r) => (mailRules = r))
  load('custom_fields', (r) => (customFields = r))
  const dateCustomFields = $derived(customFields?.filter((f) => f.data_type === 'date'))
  const users = $derived([...data.users.values()])
  const tags = $derived([...data.tags.values()])

  const tr = <T extends { name: string }>(o: T[]) => o.map((x) => ({ ...x, name: t(x.name) }))
  const triggerTypeOptions = tr(WORKFLOW_TYPE_OPTIONS)
  const sourceOptions = tr(DOCUMENT_SOURCE_OPTIONS)
  const scheduleDateFieldOptions = tr(SCHEDULE_DATE_FIELD_OPTIONS)
  const aiSuggestionFieldOptions = tr(AI_SUGGESTION_FIELD_OPTIONS)
  const matchingOptions = matchingAlgorithms().filter((a) => a.id !== MATCH_AUTO)
  const actionTypeOptions = $derived(
    tr(
      allowedActionTypes(dlg.form as any, {
        emailEnabled: !!session.get(SETTINGS_KEYS.EMAIL_ENABLED),
        remoteOcrConfigured: !!rawValue(session.settings, 'remote_ocr:configured'),
        aiEnabled: !!session.get(SETTINGS_KEYS.AI_ENABLED),
      }),
    ),
  )
  const triggerTypeName = (type: number) => triggerTypeOptions.find((x) => x.id === type)?.name ?? ''
  const actionTypeName = (type: number) => actionTypeOptions.find((x) => x.id === type)?.name ?? ''

  // NgbAccordion state (closeOthers for triggers/actions)
  const acc = `ngb-accordion-item-${accSeq++}`
  let openTriggers = $state(false)
  let openActions = $state(false)
  let openTrigger = $state<number | null>(null)
  let openAction = $state<number | null>(null)

  function addTrigger() {
    dlg.form.triggers.push(triggerForm(newTrigger()))
  }
  function addAction() {
    dlg.form.actions.push(actionForm(newAction()))
  }
  function removeTrigger(i: number) {
    dlg.form.triggers.splice(i, 1)
    if (openTrigger === i) openTrigger = null
  }
  function removeAction(i: number) {
    dlg.form.actions.splice(i, 1)
    if (openAction === i) openAction = null
  }
  let dragFrom: number | null = null
  function onActionDrop(e: DragEvent) {
    e.preventDefault()
    if (dragFrom === null) return
    const items = [...(e.currentTarget as HTMLElement).querySelectorAll(':scope > .accordion-item')]
    let to = items.findIndex((el) => e.clientY < el.getBoundingClientRect().bottom)
    if (to < 0) to = items.length - 1
    dlg.form.actions = moveItem(dlg.form.actions, dragFrom, to)
    dragFrom = null
  }

  // Filter
  function addFilter(trigger: any) {
    const type = nextFilterType(trigger.filters)
    if (type) trigger.filters.push({ type, values: defaultFilterValue(type) })
  }
  function setFilterType(filter: FilterForm, type: TriggerFilterType) {
    filter.type = type
    if (type !== 'custom_field_query') filter.values = defaultFilterValue(type)
  }
  const filterSelectItems = (type: TriggerFilterType) => {
    const d = getFilterDefinition(type)
    if (d?.inputType !== 'select') return []
    return d.selectItems === 'correspondents' ? correspondents : d.selectItems === 'documentTypes' ? documentTypes : storagePaths
  }
  // Custom field query per filter (ensureCustomFieldQueryModel)
  const cfStates = new WeakMap<FilterForm, CustomFieldsQueryDropdownState>()
  function cfState(filter: FilterForm) {
    let st = cfStates.get(filter)
    if (st) return st
    const model = new CustomFieldQueriesModel()
    if (typeof filter.values === 'string' && filter.values) {
      try {
        model.queries = [new CustomFieldQueryExpression(JSON.parse(filter.values))]
      } catch {
        model.clear(false)
        model.addInitialAtom()
      }
    }
    st = new CustomFieldsQueryDropdownState(model, { customFields: customFields ?? data.customFields })
    st.onChange((m) => {
      filter.values = !m.isValid() || m.isEmpty() ? null : JSON.stringify(m.queries[0].serialize())
    })
    cfStates.set(filter, st)
    return st
  }
  const cfValid = (filter: FilterForm) => {
    const m = cfStates.get(filter)?.selectionModel
    return !m || m.isEmpty() || m.isValid()
  }

  function removeSelectedCustomField(fieldId: number, action: any) {
    action.assign_custom_fields = (action.assign_custom_fields ?? []).filter((id: number) => id !== fieldId)
  }

  function save() {
    dlg.save(() => workflowFormValues($state.snapshot(dlg.form) as any))
  }
  const heading = (k: string) => tRich(k)
</script>

{#snippet triggerFormTpl(f: any)}
  <div>
    <input type="hidden" value={f.id ?? ''} />
    <pngx-input-select><InputSelect title={t('dialog.workflow.trigger_type')} horizontal={true} items={triggerTypeOptions} bind:value={f.type} /></pngx-input-select>
    {#if f.type === WorkflowTriggerType.Scheduled}
      <p class="small">{t('dialog.workflow.set_scheduled_trigger_offset')}</p>
      <div class="row">
        <div class="col-4">
          <pngx-input-number><InputNumber
            title={t('dialog.workflow.offset_days')}
            bind:value={f.schedule_offset_days}
            showAdd={false}
            error={dlg.error?.schedule_offset_days}
            hint={t('dialog.workflow.positive_values_trigger_after')}
          /></pngx-input-number>
        </div>
        <div class="col-4">
          <pngx-input-select><InputSelect title={t('dialog.workflow.relative')} bind:value={f.schedule_date_field} items={scheduleDateFieldOptions} error={dlg.error?.schedule_date_field} /></pngx-input-select>
        </div>
        {#if f.schedule_date_field === 'custom_field'}
          <div class="col-4">
            <pngx-input-select><InputSelect title={t('dialog.workflow.custom_field')} bind:value={f.schedule_date_custom_field} items={dateCustomFields} hint={t('dialog.workflow.custom_field_use_date')} error={dlg.error?.schedule_date_custom_field} /></pngx-input-select>
          </div>
        {/if}
      </div>
      <div class="row">
        <div class="col-4">
          <pngx-input-check><InputCheck title={t('dialog.workflow.recurring')} bind:value={f.schedule_is_recurring} hint={t('dialog.workflow.trigger_recurring')} error={dlg.error?.schedule_is_recurring} /></pngx-input-check>
        </div>
        <div class="col-4">
          {#if f.schedule_is_recurring === true}
            <pngx-input-number><InputNumber title={t('dialog.workflow.recurring_interval_days')} bind:value={f.schedule_recurring_interval_days} hint={t('dialog.workflow.repeat_trigger_every_n')} showAdd={false} error={dlg.error?.schedule_recurring_interval_days} /></pngx-input-number>
          {/if}
        </div>
      </div>
    {/if}
    <p class="small">{#each heading('dialog.workflow.trigger_documents_match_all') as seg}{#if seg.tag}<em>{seg.text}</em>{:else}{seg.text}{/if}{/each}</p>
    <div class="row">
      <div class="col">
        <pngx-input-text><InputText title={t('dialog.workflow.filter_filename')} bind:value={f.filter_filename} horizontal={true} hint={t('dialog.workflow.apply_documents_match_filename')} error={dlg.error?.filter_filename} /></pngx-input-text>
        {#if f.type === WorkflowTriggerType.Consumption}
          <pngx-input-select><InputSelect title={t('dialog.workflow.filter_sources')} items={sourceOptions} horizontal={true} multiple={true} bind:value={f.sources} error={dlg.error?.sources} /></pngx-input-select>
          <pngx-input-text><InputText title={t('dialog.workflow.filter_path')} bind:value={f.filter_path} horizontal={true} hint={t('dialog.workflow.apply_documents_match_path')} error={dlg.error?.filter_path} /></pngx-input-text>
          <pngx-input-select><InputSelect title={t('dialog.workflow.filter_mail_rule')} items={mailRules} horizontal={true} allowNull={true} bind:value={f.filter_mailrule} hint={t('dialog.workflow.apply_documents_consumed_via')} error={dlg.error?.filter_mailrule} /></pngx-input-select>
        {/if}
        {#if triggerHasDocumentFilters(f.type)}
          <pngx-input-select><InputSelect title={t('dialog.workflow.content_matching_algorithm')} horizontal={true} items={matchingOptions} bind:value={f.matching_algorithm} /></pngx-input-select>
          {#if f.matching_algorithm !== MATCH_NONE}
            <pngx-input-text><InputText title={t('dialog.workflow.content_matching_pattern')} horizontal={true} bind:value={f.match} error={dlg.error?.match} /></pngx-input-text>
          {/if}
          {#if f.matching_algorithm !== MATCH_NONE}
            <pngx-input-check><InputCheck title={t('common.case_insensitive')} horizontal={true} bind:value={f.is_insensitive} /></pngx-input-check>
          {/if}
        {/if}
      </div>
    </div>
    {#if triggerHasDocumentFilters(f.type)}
      <div class="row mt-3">
        <div class="col">
          <div class="trigger-filters mb-3">
            <div class="d-flex align-items-center">
              <span class="form-label mb-0">{t('dialog.workflow.advanced_filters')}</span>
              <button
                type="button"
                class="btn btn-sm btn-outline-primary ms-auto"
                onclick={() => addFilter(f)}
                disabled={!nextFilterType(f.filters)}
              >
                <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon><span>{t('dialog.workflow.add_filter')}</span>
              </button>
            </div>
            <ul class="mt-2 list-group filters">
              {#if f.filters.length === 0}
                <p class="text-muted small">{t('dialog.workflow.no_advanced_workflow_filters')}</p>
              {/if}
              {#each f.filters as filter, filterIndex (filter)}
                <li class="list-group-item">
                  <div class="d-flex align-items-center gap-2">
                    <div class="w-25">
                      <pngx-input-select><InputSelect
                        items={filterTypeOptions(f.filters, filterIndex).map((o) => ({ ...o, name: t(o.name) }))}
                        value={filter.type}
                        onchange={(v) => setFilterType(filter, v)}
                        allowNull={false}
                      /></pngx-input-select>
                    </div>
                    <div class="flex-grow-1">
                      {#if isTagsFilter(filter.type)}
                        <pngx-input-tags><InputTags
                          allowCreate={false}
                          title=""
                          {tags}
                          bind:value={filter.values}
                        /></pngx-input-tags>
                      {:else if isCustomFieldQueryFilter(filter.type)}
                        <pngx-custom-fields-query-dropdown><CustomFieldsQueryDropdown queryState={cfState(filter)} title="" useDropdown={false} /></pngx-custom-fields-query-dropdown>
                        {#if !cfValid(filter)}
                          <div class="text-danger small">
                            {t('dialog.workflow.complete_custom_field_query')}
                          </div>
                        {/if}
                      {:else}
                        <pngx-input-select><InputSelect
                          items={filterSelectItems(filter.type)}
                          allowNull={true}
                          multiple={isSelectMultiple(filter.type)}
                          bind:value={filter.values}
                        /></pngx-input-select>
                      {/if}
                    </div>
                    <button
                      type="button"
                      class="btn btn-link text-danger p-0"
                      onclick={() => f.filters.splice(filterIndex, 1)}
                    >
                      <pngx-bs-icon name="trash">{@html icon('trash')}</pngx-bs-icon><span class="ms-1">{t('common.delete')}</span>
                    </button>
                  </div>
                </li>
              {/each}
            </ul>
          </div>
        </div>
      </div>
    {/if}
  </div>
{/snippet}

{#snippet permRows(a: any, prefix: 'assign' | 'remove')}
  <div class="mb-2">
    <div class="row mb-1">
      <div class="col-lg-3">
        <label class="form-label d-block my-2 text-nowrap">{t('common.users_label')}</label>
      </div>
      <div class="col-lg-9">
        <pngx-permissions-user type="view"><PermissionsUser bind:value={a[`${prefix}_view_users`]} disabled={isRemovalDisabled(a, `${prefix}_view_users`)} /></pngx-permissions-user>
      </div>
    </div>
    <div class="row">
      <div class="col-lg-3">
        <label class="form-label d-block my-2 text-nowrap">{t('common.groups_label')}</label>
      </div>
      <div class="col-lg-9">
        <pngx-permissions-group type="view"><PermissionsGroup bind:value={a[`${prefix}_view_groups`]} disabled={isRemovalDisabled(a, `${prefix}_view_groups`)} /></pngx-permissions-group>
      </div>
    </div>
  </div>
{/snippet}

{#snippet permRowsChange(a: any, prefix: 'assign' | 'remove')}
  <div>
    <div class="row mb-1">
      <div class="col-lg-3">
        <label class="form-label d-block my-2 text-nowrap">{t('common.users_label')}</label>
      </div>
      <div class="col-lg-9">
        <pngx-permissions-user type="change"><PermissionsUser bind:value={a[`${prefix}_change_users`]} disabled={isRemovalDisabled(a, `${prefix}_change_users`)} /></pngx-permissions-user>
      </div>
    </div>
    <div class="row">
      <div class="col-lg-3">
        <label class="form-label d-block my-2 text-nowrap">{t('common.groups_label')}</label>
      </div>
      <div class="col-lg-9">
        <pngx-permissions-group type="change"><PermissionsGroup bind:value={a[`${prefix}_change_groups`]} disabled={isRemovalDisabled(a, `${prefix}_change_groups`)} /></pngx-permissions-group>
      </div>
    </div>
    <small class="form-text text-muted text-end d-block">{t('common.edit_permissions_also_grant')}</small>
  </div>
{/snippet}

{#snippet removal(a: any, heading: string, all: string, field: string, items: any[] | undefined, bindLabel = 'name')}
  <h6 class="form-label">{heading}</h6>
  <pngx-input-switch><InputSwitch title={t('dialog.workflow.remove_all')} horizontal={true} bind:value={a[all]} /></pngx-input-switch>
  <div class="mt-n3">
    <pngx-input-select><InputSelect title="" multiple={true} {items} {bindLabel} bind:value={a[field]} disabled={isRemovalDisabled(a, field)} /></pngx-input-select>
  </div>
{/snippet}

{#snippet actionFormTpl(a: any, i: number)}
  <div>
    <input type="hidden" value={a.id ?? ''} />
    <pngx-input-select><InputSelect title={t('dialog.workflow.action_type')} horizontal={true} items={actionTypeOptions} bind:value={a.type} /></pngx-input-select>
    {#if a.type === WorkflowActionType.Assignment}
      <div class="row">
        <div class="col">
          <pngx-input-text><InputText title={t('dialog.workflow.assign_title')} bind:value={a.assign_title} hint={t("dialog.workflow.include_some_placeholders_see")} error={dlg.error?.actions?.[i]?.assign_title} /></pngx-input-text>
          <pngx-input-tags><InputTags allowCreate={false} title={t('dialog.workflow.assign_tags')} {tags} bind:value={a.assign_tags} /></pngx-input-tags>
          <pngx-input-select><InputSelect title={t('common.assign_document_type')} items={documentTypes} allowNull={true} bind:value={a.assign_document_type} /></pngx-input-select>
          <pngx-input-select><InputSelect title={t('common.assign_correspondent')} items={correspondents} allowNull={true} bind:value={a.assign_correspondent} /></pngx-input-select>
          <pngx-input-select><InputSelect title={t('dialog.workflow.assign_storage_path')} items={storagePaths} allowNull={true} bind:value={a.assign_storage_path} /></pngx-input-select>
          <pngx-input-select><InputSelect title={t('dialog.workflow.assign_custom_fields')} multiple={true} items={customFields} allowNull={true} bind:value={a.assign_custom_fields} /></pngx-input-select>
          <pngx-input-custom-fields-values><CustomFieldsValues bind:value={a.assign_custom_fields_values} selectedFields={a.assign_custom_fields} onremoveselectedfield={(id) => removeSelectedCustomField(id, a)} /></pngx-input-custom-fields-values>
        </div>
        <div class="col">
          <pngx-input-select><InputSelect title={t('dialog.workflow.assign_owner')} items={users} bindLabel="username" bind:value={a.assign_owner} allowNull={true} /></pngx-input-select>
          <div>
            <label class="form-label">{t('dialog.workflow.assign_view_permissions')}</label>
            {@render permRows(a, 'assign')}
            <label class="form-label">{t('dialog.workflow.assign_edit_permissions')}</label>
            {@render permRowsChange(a, 'assign')}
          </div>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.Removal}
      <div class="row">
        <div class="col">
          <h6 class="form-label">{t('dialog.workflow.remove_tags')}</h6>
          <pngx-input-switch><InputSwitch title={t('dialog.workflow.remove_all')} horizontal={true} bind:value={a.remove_all_tags} /></pngx-input-switch>
          <div class="mt-n3">
            <pngx-input-tags><InputTags allowCreate={false} title="" {tags} bind:value={a.remove_tags} disabled={isRemovalDisabled(a, 'remove_tags')} /></pngx-input-tags>
          </div>

          {@render removal(a, t('dialog.workflow.remove_correspondents'), 'remove_all_correspondents', 'remove_correspondents', correspondents)}
          {@render removal(a, t('dialog.workflow.remove_document_types'), 'remove_all_document_types', 'remove_document_types', documentTypes)}
          {@render removal(a, t('dialog.workflow.remove_storage_paths'), 'remove_all_storage_paths', 'remove_storage_paths', storagePaths)}
          {@render removal(a, t('dialog.workflow.remove_custom_fields'), 'remove_all_custom_fields', 'remove_custom_fields', customFields)}
        </div>
        <div class="col">
          {@render removal(a, t('dialog.workflow.remove_owners'), 'remove_all_owners', 'remove_owners', users, 'username')}

          <h6 class="form-label">{t('dialog.workflow.remove_permissions')}</h6>
          <pngx-input-switch><InputSwitch title={t('dialog.workflow.remove_all')} horizontal={true} bind:value={a.remove_all_permissions} /></pngx-input-switch>
          <div>
            <label class="form-label">{t('dialog.workflow.view_permissions')}</label>
            {@render permRows(a, 'remove')}
            <label class="form-label">{t('common.edit_permissions')}</label>
            {@render permRowsChange(a, 'remove')}
          </div>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.Email}
      <div class="row">
        <input type="hidden" value={a.email.id ?? ''} />
        <div class="col">
          <pngx-input-text><InputText title={t('dialog.workflow.email_subject')} bind:value={a.email.subject} error={dlg.error?.actions?.[i]?.email?.subject} /></pngx-input-text>
          <pngx-input-textarea><InputTextarea title={t('dialog.workflow.email_body')} bind:value={a.email.body} error={dlg.error?.actions?.[i]?.email?.body} /></pngx-input-textarea>
          <pngx-input-text><InputText title={t('dialog.workflow.email_recipients')} bind:value={a.email.to} error={dlg.error?.actions?.[i]?.email?.to} /></pngx-input-text>
          <pngx-input-switch><InputSwitch title={t('dialog.workflow.attach_document')} bind:value={a.email.include_document} /></pngx-input-switch>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.Webhook}
      <div class="row">
        <input type="hidden" value={a.webhook.id ?? ''} />
        <div class="col">
          <pngx-input-text><InputText title={t('dialog.workflow.webhook_url')} bind:value={a.webhook.url} error={dlg.error?.actions?.[i]?.url} /></pngx-input-text>
          <div class="d-flex">
            <pngx-input-switch><InputSwitch title={t('dialog.workflow.use_parameters_webhook_body')} bind:value={a.webhook.use_params} horizontal={true} /></pngx-input-switch>
            <pngx-input-switch class="ms-5"><InputSwitch title={t('dialog.workflow.send_webhook_payload_json')} bind:value={a.webhook.as_json} horizontal={true} /></pngx-input-switch>
          </div>
          {#if a.webhook.use_params}
            <pngx-input-entries><InputEntries title={t('dialog.workflow.webhook_params')} bind:value={a.webhook.params} error={dlg.error?.actions?.[i]?.params} /></pngx-input-entries>
          {:else}
            <pngx-input-textarea><InputTextarea title={t('dialog.workflow.webhook_body')} bind:value={a.webhook.body} error={dlg.error?.actions?.[i]?.body} /></pngx-input-textarea>
          {/if}
          <pngx-input-entries><InputEntries title={t('dialog.workflow.webhook_headers')} bind:value={a.webhook.headers} error={dlg.error?.actions?.[i]?.headers} /></pngx-input-entries>
          <pngx-input-switch><InputSwitch title={t('dialog.workflow.include_document')} bind:value={a.webhook.include_document} /></pngx-input-switch>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.PasswordRemoval}
      <div class="row">
        <div class="col">
          <p class="small">
            {t('dialog.workflow.one_password_per_line')}
          </p>
          <pngx-input-textarea><InputTextarea
            title={t('dialog.workflow.passwords')}
            bind:value={a.passwords}
            error={dlg.error?.actions?.[i]?.passwords}
            hint={t('dialog.workflow.passwords_stored_in_plain')}
          /></pngx-input-textarea>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.MoveToTrash}
      <div class="row">
        <div class="col">
          <p class="text-muted small">{t('dialog.workflow.document_moved_trash_end')}</p>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.RemoteOcr}
      <div class="row">
        <div class="col">
          <p class="text-muted small">{t('dialog.workflow.document_sent_configured_remote')}</p>
        </div>
      </div>
    {:else if a.type === WorkflowActionType.ApplyAiSuggestions}
      <div class="row">
        <div class="col">
          <p class="text-muted small">{t('dialog.workflow.document_sent_configured_ai')}</p>
          <p class="text-muted small">{t('dialog.workflow.action_runs_in_background')}</p>
          <pngx-input-select><InputSelect
            title={t('dialog.workflow.apply_suggestions')}
            items={aiSuggestionFieldOptions}
            multiple={true}
            bind:value={a.ai_suggestion_fields}
            error={dlg.error?.actions?.[i]?.ai_suggestion_fields}
            hint={t('dialog.workflow.suggestions_fields_not_selected')}
          /></pngx-input-select>
        </div>
      </div>
      <div class="row">
        <div class="col-md-6">
          <pngx-input-switch><InputSwitch
            horizontal={true}
            title={t('dialog.workflow.create_missing_items')}
            bind:value={a.ai_create_missing}
            hint={t('dialog.workflow.create_suggested_tags_correspondents')}
          /></pngx-input-switch>
        </div>
        <div class="col-md-6">
          <pngx-input-switch><InputSwitch
            horizontal={true}
            title={t('dialog.workflow.overwrite_existing_values')}
            bind:value={a.ai_overwrite_existing}
            hint={t('dialog.workflow.apply_suggestions_even_document')}
          /></pngx-input-switch>
        </div>
      </div>
    {/if}
  </div>
{/snippet}

<pngx-workflow-edit-dialog><form onsubmit={(e) => { e.preventDefault(); save() }} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{title}</h4>
    {#if dlg.object?.id}
      <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {dlg.object.id}</span>
    {/if}
    <button type="button" disabled={!dlg.closeEnabled} class="btn-close" aria-label="Close" onclick={() => dlg.cancel()}>
    </button>
  </div>
  <div class="modal-body">
    <div class="row">
      <div class="col-md-6">
        <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
      </div>
      <div class="col-4">
        <pngx-input-number><InputNumber title={t('common.sort_order')} bind:value={dlg.form.order} showAdd={false} error={dlg.error?.order} /></pngx-input-number>
      </div>
      <div class="col">
        <pngx-input-switch><InputSwitch title={t('common.enabled')} bind:value={dlg.form.enabled} /></pngx-input-switch>
      </div>
    </div>
    <div class="accordion">
      <div class="accordion-item">
        <h2 role="heading" class="accordion-header">
          <button type="button" class="accordion-button" class:collapsed={!openTriggers} id="{acc}-t-toggle" aria-controls="{acc}-t-collapse" aria-expanded={openTriggers} onclick={() => (openTriggers = !openTriggers)}>{t('common.triggers')}</button>
        </h2>
        <div role="region" class="accordion-collapse collapse" class:show={openTriggers} id="{acc}-t-collapse" aria-labelledby="{acc}-t-toggle">
          <div class="accordion-body">
            {#if openTriggers}
              <div class="d-flex">
                <p class="p-2">{t('dialog.workflow.trigger_workflow_label')}</p>
                <button type="button" class="btn btn-sm btn-outline-primary ms-auto mb-3" onclick={addTrigger}>
                  <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('dialog.workflow.add_trigger')}
                </button>
              </div>
              <div class="accordion">
                {#each dlg.form.triggers as trigger, i (trigger)}
                  <div class="accordion-item">
                    <div role="heading" class="accordion-header">
                      <button type="button" class="accordion-button" class:collapsed={openTrigger !== i} aria-expanded={openTrigger === i} onclick={() => (openTrigger = openTrigger === i ? null : i)}>{i + 1}. {triggerTypeName(trigger.type)}
                        {#if trigger.id}
                          <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {trigger.id}</span>
                        {/if}
                        <pngx-confirm-button><ConfirmButton
                          label={t('common.delete')}
                          onconfirm={() => removeTrigger(i)}
                          buttonClasses="btn-link text-danger ms-2"
                          iconName="trash" /></pngx-confirm-button>
                      </button>
                    </div>
                    <div role="region" class="accordion-collapse collapse" class:show={openTrigger === i}>
                      <div class="accordion-body">
                        {#if openTrigger === i}
                          {@render triggerFormTpl(trigger)}
                        {/if}
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      </div>
      <div class="accordion-item">
        <h2 role="heading" class="accordion-header">
          <button type="button" class="btn-lg accordion-button" class:collapsed={!openActions} id="{acc}-a-toggle" aria-controls="{acc}-a-collapse" aria-expanded={openActions} onclick={() => (openActions = !openActions)}>{t('common.actions')}</button>
        </h2>
        <div role="region" class="accordion-collapse collapse" class:show={openActions} id="{acc}-a-collapse" aria-labelledby="{acc}-a-toggle">
          <div class="accordion-body">
            {#if openActions}
              <div class="d-flex">
                <p class="p-2">{t('dialog.workflow.apply_actions_label')}</p>
                <button type="button" class="btn btn-sm btn-outline-primary ms-auto mb-3" onclick={addAction}>
                  <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('dialog.workflow.add_action')}
                </button>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div class="accordion cdk-drop-list" ondragover={(e) => e.preventDefault()} ondrop={onActionDrop}>
                {#each dlg.form.actions as action, i (action)}
                  <div class="accordion-item">
                    <!-- svelte-ignore a11y_no_static_element_interactions -->
                    <div role="heading" class="accordion-header cdk-drag" draggable="true" ondragstart={() => (dragFrom = i)}>
                      <button type="button" class="accordion-button" class:collapsed={openAction !== i} aria-expanded={openAction === i} onclick={() => (openAction = openAction === i ? null : i)}>
                        <pngx-bs-icon name="grip-vertical" class="ms-n3 pe-1">{@html ricon('grip-vertical')}</pngx-bs-icon>
                        {i + 1}. {actionTypeName(action.type)}
                        {#if action.id}
                          <span class="badge bg-primary text-primary-text-contrast ms-2">ID: {action.id}</span>
                        {/if}
                        <pngx-confirm-button><ConfirmButton
                          label={t('common.delete')}
                          onconfirm={() => removeAction(i)}
                          buttonClasses="btn-link text-danger ms-2"
                          iconName="trash" /></pngx-confirm-button>
                      </button>
                    </div>
                    <div role="region" class="accordion-collapse collapse" class:show={openAction === i}>
                      <div class="accordion-body">
                        {#if openAction === i}
                          {@render actionFormTpl(action, i)}
                        {/if}
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      </div>
    </div>
  </div>
  <div class="modal-footer">
    {#if dlg.error?.non_field_errors}
      <span class="text-danger">{t('common.error')}: {dlg.error.non_field_errors}</span>
    {/if}
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-workflow-edit-dialog>
