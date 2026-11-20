<!-- All options of an email rule (like the classic MailRuleEditDialog): account, folder, filter, attachments, action
     after fetching, title/correspondent/tags/type, owner, order, "stop here". Common ones on top, rare ones
     under "Advanced". Edits the rule object directly. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import ObjectPicker from '../../components/ObjectPicker.svelte'
  import {
    ACTION_OPTIONS,
    ATTACHMENT_TYPE_OPTIONS,
    CONSUMPTION_SCOPE_OPTIONS,
    METADATA_CORRESPONDENT_OPTIONS,
    METADATA_TITLE_OPTIONS,
    PDF_LAYOUT_OPTIONS,
    showActionParamField,
    showCorrespondentField,
  } from '../../../components/manage/mail/mail-options'
  import { auto } from '../store.svelte'

  let { rule, errors = {}, idPrefix = 'au-mr' }: { rule: Record<string, any>; errors?: Record<string, any>; idPrefix?: string } = $props()
  const err = (k: string) => {
    const v = errors?.[k]
    return v == null ? null : Array.isArray(v) ? v.join(' ') : String(v)
  }
  const num = (v: string) => (v === '' ? null : Number(v))
  const id = (k: string) => `${idPrefix}-${k}`
</script>

{#snippet text(k: string, label: string, hint: string | null = null, placeholder = '')}
  <div class="au-field">
    <label for={id(k)}>{label}</label>
    <input id={id(k)} class="d2-input" {placeholder} value={rule[k] ?? ''} oninput={(e) => (rule[k] = e.currentTarget.value || null)} />
    {#if hint}<p class="au-hint">{hint}</p>{/if}
    {#if err(k)}<p class="au-err">{err(k)}</p>{/if}
  </div>
{/snippet}
{#snippet choice(k: string, label: string, options: { id: number; name: string }[], hint: string | null = null)}
  <div class="au-field">
    <label for={id(k)}>{label}</label>
    <select id={id(k)} class="d2-select" value={rule[k]} onchange={(e) => (rule[k] = Number(e.currentTarget.value))}>
      {#each options as o (o.id)}<option value={o.id}>{t(o.name)}</option>{/each}
    </select>
    {#if hint}<p class="au-hint">{hint}</p>{/if}
    {#if err(k)}<p class="au-err">{err(k)}</p>{/if}
  </div>
{/snippet}

<div class="au-form" id="{idPrefix}-form">
  <div class="au-row2">
    <div class="au-field">
      <label for={id('name')}>{t('m.auto.mail.rule_name')}</label>
      <input id={id('name')} class="d2-input" value={rule.name ?? ''} oninput={(e) => (rule.name = e.currentTarget.value)} />
      {#if err('name')}<p class="au-err">{err('name')}</p>{/if}
    </div>
    <div class="au-field">
      <label for={id('account')}>{t('common.account')}</label>
      <select id={id('account')} class="d2-select" value={rule.account ?? ''} onchange={(e) => (rule.account = num(e.currentTarget.value))}>
        <option value="" disabled>{t('m.auto.choose')}</option>
        {#each auto.mailAccounts as a (a.id)}<option value={a.id}>{a.name}</option>{/each}
      </select>
      {#if !auto.mailAccounts.length}<p class="au-hint">{t('m.auto.mail.no_accounts')}</p>{/if}
      {#if err('account')}<p class="au-err">{err('account')}</p>{/if}
    </div>
  </div>
  <label class="au-check"><input type="checkbox" id={id('enabled')} checked={rule.enabled} onchange={(e) => (rule.enabled = e.currentTarget.checked)} />{t('m.auto.mail.rule_enabled')}</label>

  <section class="au-group" aria-labelledby="{idPrefix}-which">
    <h3 id="{idPrefix}-which">{t('m.auto.mail.which_mails')}</h3>
    {@render text('folder', t('common.folder'), t('dialog.mail_rule.subfolders_must_separated_delimiter'))}
    <div class="au-row2">
      {@render text('filter_from', t('dialog.mail_rule.filter'))}
      {@render text('filter_subject', t('dialog.mail_rule.filter_subject'))}
    </div>
    <div class="au-row2">
      {@render text('filter_to', t('dialog.mail_rule.filter_2'))}
      {@render text('filter_body', t('dialog.mail_rule.filter_body'))}
    </div>
    <div class="au-field">
      <label for={id('maximum_age')}>{t('dialog.mail_rule.maximum_age_days')}</label>
      <input id={id('maximum_age')} class="d2-input" type="number" min="0" value={rule.maximum_age ?? ''} oninput={(e) => (rule.maximum_age = num(e.currentTarget.value))} />
      {#if err('maximum_age')}<p class="au-err">{err('maximum_age')}</p>{/if}
    </div>
  </section>

  <section class="au-group" aria-labelledby="{idPrefix}-what">
    <h3 id="{idPrefix}-what">{t('m.auto.mail.what_import')}</h3>
    {@render choice('consumption_scope', t('dialog.mail_rule.consumption_scope'), CONSUMPTION_SCOPE_OPTIONS, t('dialog.mail_rule.see_docs_eml_processing'))}
    {@render choice('attachment_type', t('dialog.mail_rule.attachment_type'), ATTACHMENT_TYPE_OPTIONS)}
    <div class="au-row2">
      {@render text('filter_attachment_filename_include', t('dialog.mail_rule.include_only_files_matching'), t('dialog.mail_rule.optional_wildcards_e_g'))}
      {@render text('filter_attachment_filename_exclude', t('dialog.mail_rule.exclude_files_matching'), t('dialog.mail_rule.optional_wildcards_e_g'))}
    </div>
  </section>

  <section class="au-group" aria-labelledby="{idPrefix}-after">
    <h3 id="{idPrefix}-after">{t('m.auto.mail.after_fetch')}</h3>
    {@render choice('action', t('common.action'), ACTION_OPTIONS, t('dialog.mail_rule.only_performed_mail_processed'))}
    {#if showActionParamField(rule.action)}{@render text('action_parameter', t('dialog.mail_rule.action_parameter'))}{/if}
  </section>

  <section class="au-group" aria-labelledby="{idPrefix}-doc">
    <h3 id="{idPrefix}-doc">{t('m.auto.mail.document_values')}</h3>
    {@render choice('assign_title_from', t('dialog.mail_rule.assign_title'), METADATA_TITLE_OPTIONS)}
    {@render choice('assign_correspondent_from', t('dialog.mail_rule.assign_correspondent'), METADATA_CORRESPONDENT_OPTIONS)}
    {#if showCorrespondentField(rule.assign_correspondent_from)}
      <div class="au-field">
        <span class="lab">{t('common.assign_correspondent')}</span>
        <ObjectPicker id={id('assign_correspondent')} label={t('common.assign_correspondent')} options={auto.options('correspondent')} selected={rule.assign_correspondent == null ? [] : [rule.assign_correspondent]} onchange={(n) => (rule.assign_correspondent = n[0] ?? null)} placeholder={t('m.auto.choose')} />
        {#if err('assign_correspondent')}<p class="au-err">{err('assign_correspondent')}</p>{/if}
      </div>
    {/if}
    <div class="au-field">
      <span class="lab">{t('dialog.workflow.assign_tags')}</span>
      <ObjectPicker id={id('assign_tags')} multi label={t('dialog.workflow.assign_tags')} options={auto.options('tag')} selected={rule.assign_tags ?? []} onchange={(n) => (rule.assign_tags = n.filter((x) => x !== null))} placeholder={t('m.auto.none_selected')} />
    </div>
    <div class="au-field">
      <span class="lab">{t('common.assign_document_type')}</span>
      <ObjectPicker id={id('assign_document_type')} label={t('common.assign_document_type')} options={auto.options('document_type')} selected={rule.assign_document_type == null ? [] : [rule.assign_document_type]} onchange={(n) => (rule.assign_document_type = n[0] ?? null)} placeholder={t('m.auto.none_selected')} />
    </div>
    <label class="au-check"><input type="checkbox" id={id('assign_owner_from_rule')} checked={rule.assign_owner_from_rule} onchange={(e) => (rule.assign_owner_from_rule = e.currentTarget.checked)} />{t('dialog.mail_rule.assign_owner_rule')}</label>
  </section>

  <details class="au-details" open={!!(err('order') || err('pdf_layout') || rule.stop_processing || (rule.pdf_layout ?? 0) !== 0)}>
    <summary>{t('m.auto.advanced')}</summary>
    <div class="au-form">
      {@render choice('pdf_layout', t('dialog.mail_rule.pdf_layout'), PDF_LAYOUT_OPTIONS)}
      <div class="au-field">
        <label for={id('order')}>{t('dialog.mail_rule.order')}</label>
        <input id={id('order')} class="d2-input" type="number" value={rule.order ?? 0} oninput={(e) => (rule.order = num(e.currentTarget.value) ?? 0)} />
        {#if err('order')}<p class="au-err">{err('order')}</p>{/if}
      </div>
      <label class="au-check"><input type="checkbox" id={id('stop_processing')} checked={rule.stop_processing} onchange={(e) => (rule.stop_processing = e.currentTarget.checked)} /><span>{t('dialog.mail_rule.stop_further_processing')}<br /><span class="au-hint">{t('dialog.mail_rule.stop_processing_further_rules')}</span></span></label>
    </div>
  </details>
  {#if err('non_field_errors')}<p class="au-err">{err('non_field_errors')}</p>{/if}
</div>
