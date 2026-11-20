<!-- Edit email flow (topic 11 B): mail rule (all options) + the workflows that run via this rule on "Email arrives".
     Saved via /rs/api/v2/automation/mail-flows (one transaction; the rule is validated like
     /api/mail_rules/). The workflow editor edits workflows. -->
<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { deleteOf } from '../../api/admin'
  import Icon from '../components/Icon.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import MailRuleForm from './panels/MailRuleForm.svelte'
  import { createMailFlow, errorOf, getMailFlow, patchMailFlow, type CombinedWorkflow } from './api'
  import { emptyDraft, fullTrigger, newMailRule, SOURCE_MAIL, WorkflowTriggerType } from './model'
  import { sentenceText } from './sentence'
  import { auto } from './store.svelte'

  let { id }: { id: string | null } = $props()
  const isNew = untrack(() => id === 'new' || id === null)
  const loc = locale()
  let rule = $state<Record<string, any> | null>(null)
  let savedJson = $state('')
  let workflows = $state<CombinedWorkflow[]>([])
  let failed = $state(false)
  let saving = $state(false)
  let errors = $state<Record<string, any>>({})
  let problem = $state<string | null>(null)
  let confirmDelete = $state(false)
  const canChange = session.can(isNew ? 'add' : 'change', 'mailrule')

  onMount(() => {
    void (async () => {
      await auto.load()
      if (isNew) {
        rule = newMailRule(auto.mailAccounts[0]?.id ?? null)
        return
      }
      try {
        const r = await getMailFlow(Number(id))
        rule = r.mail_rule
        savedJson = JSON.stringify(r.mail_rule)
        workflows = r.workflows
      } catch {
        failed = true
      }
    })()
  })
  const dirty = $derived(!!rule && JSON.stringify(rule) !== savedJson)

  async function save() {
    if (!rule) return
    problem = null
    errors = {}
    if (!rule.name?.trim()) return void (problem = t('m.auto.problem.no_rule_name'))
    if (!rule.account) return void (problem = t('m.auto.problem.no_account'))
    saving = true
    try {
      const r = isNew ? await createMailFlow({ mail_rule: rule, workflows: [] }) : await patchMailFlow(Number(id), { mail_rule: rule })
      rule = r.mail_rule
      savedJson = JSON.stringify(r.mail_rule)
      workflows = r.workflows
      toastService.show({ content: t('manage.mail.saved_rule', { PH: r.mail_rule.name }), delay: 3000, level: 'success' })
      void auto.load(true)
      if (isNew) router.navigate(`automation/mail/${r.mail_rule.id}`, true)
    } catch (e) {
      const err = errorOf(e)
      if (err.code === 'invalid' && err.fields) {
        errors = typeof err.fields.mail_rule === 'object' && !Array.isArray(err.fields.mail_rule) ? err.fields.mail_rule : err.fields
        problem = t('m.auto.fix_marked')
      } else problem = err.status === 403 ? t('m.auto.not_allowed') : err.message || t('manage.mail.error_saving_rule')
    } finally {
      saving = false
    }
  }

  function addWorkflow() {
    if (!rule?.id) return
    const d = emptyDraft(t('m.auto.mail.workflow_name', { PH: rule.name }))
    d.workflow.triggers = [fullTrigger({ type: WorkflowTriggerType.Consumption, sources: [SOURCE_MAIL], filter_mailrule: rule.id })]
    auto.pending = { draft: d }
    router.navigate('automation/workflows/new')
  }
  async function remove() {
    if (!rule?.id) return
    try {
      await deleteOf('mail_rules', rule.id)
      toastService.show({ content: t('manage.mail.deleted_mail_rule', { PH: rule.name }), delay: 4000 })
      savedJson = JSON.stringify(rule)
      void auto.load(true)
      router.navigate('automation/mail')
    } catch (e) {
      toastService.showError(t('manage.mail.error_deleting_mail_rule', { PH: rule.name }), e)
    } finally {
      confirmDelete = false
    }
  }
</script>

<div class="au-page" id="au-mail-editor">
  <div class="au-editor-head">
    <a class="d2-btn ghost icon" href={router.href('automation/mail')} aria-label={t('m.auto.back')} title={t('m.auto.back')}><Icon name="arrow-left" /></a>
    <h1 class="au-title">{isNew ? t('m.auto.mail.new_flow') : (rule?.name ?? '')}</h1>
    <span class="au-save-state" aria-live="polite">{saving ? t('m.auto.saving') : dirty ? t('m.auto.unsaved') : isNew ? '' : t('m.auto.all_saved')}</span>
    {#if !isNew && session.can('delete', 'mailrule')}<button type="button" class="d2-btn ghost" id="au-mail-delete" onclick={() => (confirmDelete = true)}><Icon name="trash" />{t('common.delete')}</button>{/if}
    {#if canChange}<button type="button" class="d2-btn primary" id="au-mail-save" disabled={saving || (!dirty && !isNew)} onclick={save}>{t('common.save')}</button>{/if}
  </div>
  <div class="au-editor-body d2-scroll">
    <div class="au-editor-inner">
      {#if failed}
        <div class="au-banner danger" role="alert">{t('m.auto.load_failed')} <button type="button" class="d2-btn sm" onclick={() => location.reload()}>{t('m.auto.retry')}</button></div>
      {:else if !rule}
        <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
      {:else}
        {#if problem}<div class="au-banner danger" role="alert">{problem}</div>{/if}
        <p class="au-lead">{t('m.auto.mail.editor_intro')}</p>
        <MailRuleForm {rule} {errors} idPrefix="au-mf" />

        <h2 class="au-section-title">{t('m.auto.mail.then_workflows')}</h2>
        {#if isNew}
          <p class="au-hint">{t('m.auto.mail.save_rule_first')}</p>
        {:else}
          {#if workflows.length}
            <ul class="au-list" id="au-mail-workflows">
              {#each workflows as w (w.workflow.id)}
                <li class="au-row" data-cols="3" class:off={!w.workflow.enabled}>
                  <Icon name="diagram-3" />
                  <div class="main">
                    <a class="name" href={router.href(`automation/workflows/${w.workflow.id}`)}>{w.workflow.name}</a>
                    <p class="sent">{sentenceText(w.summary, t, loc)}</p>
                  </div>
                  <a class="d2-btn sm" href={router.href(`automation/workflows/${w.workflow.id}`)}>{t('common.edit')}</a>
                </li>
              {/each}
            </ul>
          {:else}
            <p class="au-hint">{t('m.auto.mail.no_workflows_hint')}</p>
          {/if}
          {#if session.can('add', 'workflow')}
            <div class="au-add-row"><button type="button" class="d2-btn" id="au-mail-add-workflow" disabled={dirty} onclick={addWorkflow}><Icon name="plus-lg" />{t('m.auto.mail.add_workflow')}</button></div>
            {#if dirty}<p class="au-hint">{t('m.auto.mail.save_rule_first')}</p>{/if}
          {/if}
        {/if}
      {/if}
    </div>
  </div>
</div>

{#if confirmDelete && rule}
  <ConfirmDialog title={t('manage.mail.confirm_delete_mail_rule')} confirmLabel={t('common.delete')} danger onconfirm={remove} oncancel={() => (confirmDelete = false)}>
    <p>{t('manage.mail.operation_permanently_delete_mail_2')}</p>
    {#if workflows.length}<p>{t('m.auto.mail.delete_keeps_workflows', { N: workflows.length })}</p>{/if}
  </ConfirmDialog>
{/if}
