<!-- Side sheet "Then"/"Else": all action kinds and fields of the ngx workflow model (like the classic
     WorkflowEditDialog, part "Actions"): set values, remove values, email, webhook, remove password,
     trash, external OCR, AI suggestions (only with AI). Edits the action object directly. -->
<script lang="ts">
  import { tick } from 'svelte'
  import { t } from '../../../lib/i18n'
  import Icon from '../../components/Icon.svelte'
  import ObjectPicker from '../../components/ObjectPicker.svelte'
  import Entries from './Entries.svelte'
  import CfValues from './CfValues.svelte'
  import { WorkflowActionType, type NameKind } from '../model'
  import { auto } from '../store.svelte'

  let {
    action,
    index,
    branch,
    types,
    focus = null,
    errors = [],
  }: {
    action: Record<string, any>
    index: number
    branch: 'main' | 'else'
    /** allowed action kinds (id + name key) */
    types: { id: number; name: string }[]
    focus?: string | null
    errors?: string[]
  } = $props()

  const pid = $derived(`au-act-${branch}-${index}`)
  const clean = (n: (number | null)[]) => n.filter((x): x is number => x !== null)
  $effect(() => {
    if (focus) void tick().then(() => document.getElementById(`${pid}-${focus}`)?.focus())
  })

  const AI_FIELDS = [
    { id: 'title', label: 'common.title' },
    { id: 'tags', label: 'common.tags' },
    { id: 'correspondent', label: 'common.correspondent' },
    { id: 'document_type', label: 'common.document_type' },
    { id: 'storage_path', label: 'common.storage_path' },
    { id: 'created', label: 'dialog.workflow.created_date' },
  ]
  function toggleAiField(f: string, on: boolean) {
    const s = new Set<string>(action.ai_suggestion_fields ?? [])
    if (on) s.add(f)
    else s.delete(f)
    action.ai_suggestion_fields = AI_FIELDS.map((x) => x.id).filter((x) => s.has(x))
  }
  const passwordsText = $derived(Array.isArray(action.passwords) ? action.passwords.join('\n') : (action.passwords ?? ''))
  // classic text contains an HTML link → here as plain text
  const placeholdersHint = $derived(t('dialog.workflow.include_some_placeholders_see').replace(/<[^>]*>/g, ''))
</script>

{#snippet picker(field: string, label: string, kind: NameKind, multi: boolean, disabled = false)}
  <div class="au-field">
    <span class="lab">{label}</span>
    <ObjectPicker
      id="{pid}-{field}"
      {multi}
      {label}
      {disabled}
      options={auto.options(kind)}
      selected={multi ? (action[field] ?? []) : action[field] == null ? [] : [action[field]]}
      onchange={(n) => (action[field] = multi ? clean(n) : (clean(n)[0] ?? null))}
      placeholder={t('m.auto.none_selected')}
    />
  </div>
{/snippet}
{#snippet removal(all: string, field: string, label: string, kind: NameKind)}
  <div class="au-group">
    <h3>{label}</h3>
    <label class="au-check"><input type="checkbox" id="{pid}-{all}" checked={!!action[all]} onchange={(e) => (action[all] = e.currentTarget.checked)} />{t('dialog.workflow.remove_all')}</label>
    {@render picker(field, label, kind, true, !!action[all])}
  </div>
{/snippet}

<div class="au-form" id="{pid}-panel">
  {#if errors.length}<div class="au-banner danger" role="alert"><ul>{#each errors as e, i (i)}<li>{e}</li>{/each}</ul></div>{/if}
  <div class="au-field">
    <label for="{pid}-type">{t('m.auto.act.what')}</label>
    <select id="{pid}-type" class="d2-select" value={action.type} onchange={(e) => (action.type = Number(e.currentTarget.value))}>
      {#each types as o (o.id)}<option value={o.id}>{t(o.name)}</option>{/each}
    </select>
    {#if !auto.aiOn}<p class="au-hint">{t('m.auto.ai.off_hint')}</p>{/if}
  </div>

  {#if action.type === WorkflowActionType.Assignment}
    <div class="au-field">
      <label for="{pid}-assign_title">{t('dialog.workflow.assign_title')}</label>
      <input id="{pid}-assign_title" class="d2-input" value={action.assign_title ?? ''} oninput={(e) => (action.assign_title = e.currentTarget.value || null)} />
      <p class="au-hint">{placeholdersHint} {t('m.auto.act.title_example')}</p>
    </div>
    {@render picker('assign_tags', t('dialog.workflow.assign_tags'), 'tag', true)}
    {@render picker('assign_correspondent', t('common.assign_correspondent'), 'correspondent', false)}
    {@render picker('assign_document_type', t('common.assign_document_type'), 'document_type', false)}
    {@render picker('assign_storage_path', t('dialog.workflow.assign_storage_path'), 'storage_path', false)}
    {@render picker('assign_custom_fields', t('dialog.workflow.assign_custom_fields'), 'custom_field', true)}
    <CfValues fields={action.assign_custom_fields ?? []} values={action.assign_custom_fields_values ?? {}} idPrefix={pid} onchange={(v) => (action.assign_custom_fields_values = v)} />
    <details class="au-details" open={!!(action.assign_owner || action.assign_view_users?.length || action.assign_view_groups?.length || action.assign_change_users?.length || action.assign_change_groups?.length || (focus && /owner|view|change/.test(focus)))}>
      <summary id="{pid}-perm-toggle">{t('m.auto.act.owner_permissions')}</summary>
      <div class="au-form">
        {@render picker('assign_owner', t('dialog.workflow.assign_owner'), 'user', false)}
        <div class="au-group">
          <h3>{t('dialog.workflow.assign_view_permissions')}</h3>
          {@render picker('assign_view_users', t('m.auto.users'), 'user', true)}
          {@render picker('assign_view_groups', t('m.auto.groups'), 'group', true)}
        </div>
        <div class="au-group">
          <h3>{t('dialog.workflow.assign_edit_permissions')}</h3>
          {@render picker('assign_change_users', t('m.auto.users'), 'user', true)}
          {@render picker('assign_change_groups', t('m.auto.groups'), 'group', true)}
          <p class="au-hint">{t('common.edit_permissions_also_grant')}</p>
        </div>
      </div>
    </details>
  {:else if action.type === WorkflowActionType.Removal}
    {@render removal('remove_all_tags', 'remove_tags', t('dialog.workflow.remove_tags'), 'tag')}
    {@render removal('remove_all_correspondents', 'remove_correspondents', t('dialog.workflow.remove_correspondents'), 'correspondent')}
    {@render removal('remove_all_document_types', 'remove_document_types', t('dialog.workflow.remove_document_types'), 'document_type')}
    {@render removal('remove_all_storage_paths', 'remove_storage_paths', t('dialog.workflow.remove_storage_paths'), 'storage_path')}
    {@render removal('remove_all_custom_fields', 'remove_custom_fields', t('dialog.workflow.remove_custom_fields'), 'custom_field')}
    <details class="au-details" open={!!(action.remove_all_owners || action.remove_owners?.length || action.remove_all_permissions || action.remove_view_users?.length || action.remove_view_groups?.length || action.remove_change_users?.length || action.remove_change_groups?.length)}>
      <summary id="{pid}-perm-toggle">{t('m.auto.act.owner_permissions')}</summary>
      <div class="au-form">
        {@render removal('remove_all_owners', 'remove_owners', t('dialog.workflow.remove_owners'), 'user')}
        <div class="au-group">
          <h3>{t('dialog.workflow.remove_permissions')}</h3>
          <label class="au-check"><input type="checkbox" id="{pid}-remove_all_permissions" checked={!!action.remove_all_permissions} onchange={(e) => (action.remove_all_permissions = e.currentTarget.checked)} />{t('dialog.workflow.remove_all')}</label>
          <span class="lab">{t('dialog.workflow.view_permissions')}</span>
          {@render picker('remove_view_users', t('m.auto.users'), 'user', true, !!action.remove_all_permissions)}
          {@render picker('remove_view_groups', t('m.auto.groups'), 'group', true, !!action.remove_all_permissions)}
          <span class="lab">{t('common.edit_permissions')}</span>
          {@render picker('remove_change_users', t('m.auto.users'), 'user', true, !!action.remove_all_permissions)}
          {@render picker('remove_change_groups', t('m.auto.groups'), 'group', true, !!action.remove_all_permissions)}
        </div>
      </div>
    </details>
  {:else if action.type === WorkflowActionType.Email}
    {#if !auto.emailOn}<div class="au-banner warn">{t('m.auto.act.email_off')}</div>{/if}
    <div class="au-field">
      <label for="{pid}-email_to">{t('dialog.workflow.email_recipients')}</label>
      <input id="{pid}-email_to" class="d2-input" type="text" value={action.email.to ?? ''} oninput={(e) => (action.email.to = e.currentTarget.value)} />
      <p class="au-hint">{t('m.auto.act.email_to_hint')}</p>
    </div>
    <div class="au-field">
      <label for="{pid}-email_subject">{t('dialog.workflow.email_subject')}</label>
      <input id="{pid}-email_subject" class="d2-input" value={action.email.subject ?? ''} oninput={(e) => (action.email.subject = e.currentTarget.value)} />
    </div>
    <div class="au-field">
      <label for="{pid}-email_body">{t('dialog.workflow.email_body')}</label>
      <textarea id="{pid}-email_body" class="d2-textarea" rows="5" value={action.email.body ?? ''} oninput={(e) => (action.email.body = e.currentTarget.value)}></textarea>
      <p class="au-hint">{placeholdersHint}</p>
    </div>
    <label class="au-check"><input type="checkbox" id="{pid}-email_include_document" checked={!!action.email.include_document} onchange={(e) => (action.email.include_document = e.currentTarget.checked)} />{t('dialog.workflow.attach_document')}</label>
  {:else if action.type === WorkflowActionType.Webhook}
    <div class="au-field">
      <label for="{pid}-webhook_url">{t('dialog.workflow.webhook_url')}</label>
      <input id="{pid}-webhook_url" class="d2-input" type="url" placeholder="https://" value={action.webhook.url ?? ''} oninput={(e) => (action.webhook.url = e.currentTarget.value)} />
    </div>
    <label class="au-check"><input type="checkbox" id="{pid}-webhook_use_params" checked={action.webhook.use_params !== false} onchange={(e) => (action.webhook.use_params = e.currentTarget.checked)} />{t('dialog.workflow.use_parameters_webhook_body')}</label>
    <label class="au-check"><input type="checkbox" id="{pid}-webhook_as_json" checked={!!action.webhook.as_json} onchange={(e) => (action.webhook.as_json = e.currentTarget.checked)} />{t('dialog.workflow.send_webhook_payload_json')}</label>
    {#if action.webhook.use_params !== false}
      <Entries id="{pid}-webhook_params" label={t('dialog.workflow.webhook_params')} value={action.webhook.params} onchange={(v) => (action.webhook.params = v)} />
    {:else}
      <div class="au-field">
        <label for="{pid}-webhook_body">{t('dialog.workflow.webhook_body')}</label>
        <textarea id="{pid}-webhook_body" class="d2-textarea" rows="5" value={action.webhook.body ?? ''} oninput={(e) => (action.webhook.body = e.currentTarget.value)}></textarea>
        <p class="au-hint">{placeholdersHint}</p>
      </div>
    {/if}
    <Entries id="{pid}-webhook_headers" label={t('dialog.workflow.webhook_headers')} value={action.webhook.headers} onchange={(v) => (action.webhook.headers = v)} />
    <label class="au-check"><input type="checkbox" id="{pid}-webhook_include_document" checked={!!action.webhook.include_document} onchange={(e) => (action.webhook.include_document = e.currentTarget.checked)} />{t('dialog.workflow.include_document')}</label>
  {:else if action.type === WorkflowActionType.PasswordRemoval}
    <div class="au-field">
      <label for="{pid}-passwords">{t('dialog.workflow.passwords')}</label>
      <textarea id="{pid}-passwords" class="d2-textarea" rows="4" autocomplete="off" spellcheck="false" value={passwordsText} oninput={(e) => (action.passwords = e.currentTarget.value)}></textarea>
      <p class="au-hint">{t('dialog.workflow.one_password_per_line')}</p>
      <p class="au-hint"><Icon name="exclamation-triangle" /> {t('dialog.workflow.passwords_stored_in_plain')}</p>
    </div>
  {:else if action.type === WorkflowActionType.MoveToTrash}
    <p class="au-hint">{t('dialog.workflow.document_moved_trash_end')}</p>
  {:else if action.type === WorkflowActionType.RemoteOcr}
    <p class="au-hint">{t('dialog.workflow.document_sent_configured_remote')}</p>
  {:else if action.type === WorkflowActionType.ApplyAiSuggestions}
    {#if !auto.aiOn}<div class="au-banner warn">{t('m.auto.ai.action_off')}</div>{/if}
    <p class="au-hint">{t('dialog.workflow.document_sent_configured_ai')}</p>
    <p class="au-hint">{t('dialog.workflow.action_runs_in_background')}</p>
    <fieldset class="au-field" id="{pid}-ai_suggestion_fields" data-plain="">
      <legend class="lab">{t('dialog.workflow.apply_suggestions')}</legend>
      {#each AI_FIELDS as f (f.id)}
        <label class="au-check"><input type="checkbox" checked={(action.ai_suggestion_fields ?? []).includes(f.id)} onchange={(e) => toggleAiField(f.id, e.currentTarget.checked)} />{t(f.label)}</label>
      {/each}
      <p class="au-hint">{t('dialog.workflow.suggestions_fields_not_selected')}</p>
    </fieldset>
    <label class="au-check"><input type="checkbox" id="{pid}-ai_create_missing" checked={!!action.ai_create_missing} onchange={(e) => (action.ai_create_missing = e.currentTarget.checked)} /><span>{t('dialog.workflow.create_missing_items')}<br /><span class="au-hint">{t('dialog.workflow.create_suggested_tags_correspondents')}</span></span></label>
    <label class="au-check"><input type="checkbox" id="{pid}-ai_overwrite_existing" checked={!!action.ai_overwrite_existing} onchange={(e) => (action.ai_overwrite_existing = e.currentTarget.checked)} /><span>{t('dialog.workflow.overwrite_existing_values')}<br /><span class="au-hint">{t('dialog.workflow.apply_suggestions_even_document')}</span></span></label>
    <p class="au-hint">{t('m.auto.ai.review_hint')}</p>
  {/if}
</div>
