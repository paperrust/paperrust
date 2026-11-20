<!-- Side sheet "When": all trigger types and fields (like the classic WorkflowEditDialog, part "Triggers"),
     plus "Email arrives" (topic 11 B): ingestion trigger with mail rule – choose a rule or create a new one, all
     options of the rule here. Conditions of the trigger are in the "Only if" sheet. -->
<script lang="ts">
  import { t } from '../../../lib/i18n'
  import { data } from '../../../stores/data.svelte'
  import { session } from '../../../stores/settings.svelte'
  import MailRuleForm from './MailRuleForm.svelte'
  import { SOURCE_MAIL, WorkflowTriggerType } from '../model'
  import { auto } from '../store.svelte'

  let {
    trigger,
    index,
    errors = [],
    mailRule = null,
    mailErrors = {},
    onpickrule,
  }: {
    trigger: Record<string, any>
    index: number
    errors?: string[]
    mailRule?: Record<string, any> | null
    mailErrors?: Record<string, any>
    /** Mail rule chosen (id) or new (-1) – the editor loads or creates the rule object */
    onpickrule: (id: number | null) => void
  } = $props()

  type Kind = 'added' | 'updated' | 'consumption' | 'mail' | 'scheduled' | 'manual'
  const kind = $derived<Kind>(
    trigger.type === WorkflowTriggerType.Consumption
      ? trigger.filter_mailrule != null
        ? 'mail'
        : 'consumption'
      : trigger.type === WorkflowTriggerType.DocumentUpdated
        ? 'updated'
        : trigger.type === WorkflowTriggerType.Scheduled
          ? 'scheduled'
          : trigger.type === WorkflowTriggerType.Manual
            ? 'manual'
            : 'added',
  )
  const canMail = session.can('view', 'mailrule')
  const KINDS: { k: Kind; label: string; hint: string }[] = [
    { k: 'added', label: 'm.auto.trig.added', hint: 'm.auto.trig.added_hint' },
    { k: 'updated', label: 'm.auto.trig.updated', hint: 'm.auto.trig.updated_hint' },
    { k: 'consumption', label: 'm.auto.trig.consumption', hint: 'm.auto.trig.consumption_hint' },
    { k: 'mail', label: 'm.auto.trig.mail', hint: 'm.auto.trig.mail_hint' },
    { k: 'scheduled', label: 'm.auto.trig.scheduled', hint: 'm.auto.trig.scheduled_hint' },
    { k: 'manual', label: 'm.auto.trig.manual', hint: 'm.auto.trig.manual_hint' },
  ]
  const kinds = $derived(KINDS.filter((x) => x.k !== 'mail' || canMail || kind === 'mail'))

  function setKind(k: Kind) {
    if (k === kind) return
    const wasMail = kind === 'mail'
    trigger.type = { added: 2, updated: 3, consumption: 1, mail: 1, scheduled: 4, manual: 5 }[k]
    if (k === 'mail') {
      const first = auto.mailRules[0]?.id ?? -1
      trigger.filter_mailrule = first
      if (Array.isArray(trigger.sources) && trigger.sources.length && !trigger.sources.includes(SOURCE_MAIL)) trigger.sources = [...trigger.sources, SOURCE_MAIL]
      onpickrule(first)
    } else if (wasMail) {
      trigger.filter_mailrule = null
      onpickrule(null)
    }
  }
  const num = (v: string, d: number) => (v === '' || Number.isNaN(Number(v)) ? d : Number(v))
  const dateFields = $derived(data.customFields.filter((f) => f.data_type === 'date'))
  const pid = $derived(`au-trig-${index}`)
</script>

<div class="au-form" id="{pid}-panel">
  {#if errors.length}<div class="au-banner danger" role="alert"><ul>{#each errors as e, i (i)}<li>{e}</li>{/each}</ul></div>{/if}
  <fieldset class="au-field" data-plain="">
    <legend class="lab au-legend">{t('m.auto.trig.question')}</legend>
    {#each kinds as k (k.k)}
      <label class="au-check" data-radio="">
        <input type="radio" name="{pid}-kind" id="{pid}-kind-{k.k}" value={k.k} checked={kind === k.k} onchange={() => setKind(k.k)} />
        <span><strong>{t(k.label)}</strong><br /><span class="au-hint">{t(k.hint)}</span></span>
      </label>
    {/each}
  </fieldset>

  {#if kind === 'scheduled'}
    <section class="au-group" aria-labelledby="{pid}-sched">
      <h3 id="{pid}-sched">{t('m.auto.trig.when_exactly')}</h3>
      <p class="au-hint">{t('dialog.workflow.set_scheduled_trigger_offset')}</p>
      <div class="au-row2">
        <div class="au-field">
          <label for="{pid}-offset">{t('dialog.workflow.offset_days')}</label>
          <input id="{pid}-offset" class="d2-input" type="number" value={trigger.schedule_offset_days ?? 0} oninput={(e) => (trigger.schedule_offset_days = num(e.currentTarget.value, 0))} />
          <p class="au-hint">{t('dialog.workflow.positive_values_trigger_after')}</p>
        </div>
        <div class="au-field">
          <label for="{pid}-datefield">{t('dialog.workflow.relative')}</label>
          <select id="{pid}-datefield" class="d2-select" value={trigger.schedule_date_field ?? 'added'} onchange={(e) => (trigger.schedule_date_field = e.currentTarget.value)}>
            <option value="added">{t('common.added')}</option>
            <option value="created">{t('common.created')}</option>
            <option value="modified">{t('common.modified')}</option>
            <option value="custom_field">{t('dialog.workflow.custom_field')}</option>
          </select>
        </div>
      </div>
      {#if trigger.schedule_date_field === 'custom_field'}
        <div class="au-field">
          <label for="{pid}-cf">{t('dialog.workflow.custom_field')}</label>
          <select id="{pid}-cf" class="d2-select" value={trigger.schedule_date_custom_field ?? ''} onchange={(e) => (trigger.schedule_date_custom_field = e.currentTarget.value === '' ? null : Number(e.currentTarget.value))}>
            <option value="">{t('m.auto.choose')}</option>
            {#each dateFields as f (f.id)}<option value={f.id}>{f.name}</option>{/each}
          </select>
          <p class="au-hint">{t('dialog.workflow.custom_field_use_date')}</p>
        </div>
      {/if}
      <label class="au-check"><input type="checkbox" id="{pid}-recurring" checked={!!trigger.schedule_is_recurring} onchange={(e) => (trigger.schedule_is_recurring = e.currentTarget.checked)} />{t('m.auto.trig.recurring')}</label>
      {#if trigger.schedule_is_recurring}
        <div class="au-field">
          <label for="{pid}-interval">{t('dialog.workflow.recurring_interval_days')}</label>
          <input id="{pid}-interval" class="d2-input" type="number" min="1" value={trigger.schedule_recurring_interval_days ?? 1} oninput={(e) => (trigger.schedule_recurring_interval_days = num(e.currentTarget.value, 1))} />
          <p class="au-hint">{t('dialog.workflow.repeat_trigger_every_n')}</p>
        </div>
      {/if}
    </section>
  {/if}

  {#if kind === 'mail'}
    <section class="au-group" aria-labelledby="{pid}-mail">
      <h3 id="{pid}-mail">{t('m.auto.trig.mail_rule')}</h3>
      <div class="au-field">
        <label for="{pid}-rule">{t('m.auto.trig.which_rule')}</label>
        <select id="{pid}-rule" class="d2-select" value={trigger.filter_mailrule ?? -1} onchange={(e) => {
          const v = Number(e.currentTarget.value)
          trigger.filter_mailrule = v
          onpickrule(v)
        }}>
          {#each auto.mailRules as r (r.id)}<option value={r.id}>{r.name}</option>{/each}
          {#if session.can('add', 'mailrule')}<option value={-1}>{t('m.auto.trig.new_rule')}</option>{/if}
        </select>
        <p class="au-hint">{t('m.auto.trig.rule_shared_hint')}</p>
      </div>
      {#if mailRule}
        <MailRuleForm rule={mailRule} errors={mailErrors} idPrefix="{pid}-mr" />
      {:else}
        <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
      {/if}
    </section>
  {/if}
  {#if kind === 'consumption'}
    <p class="au-hint">{t('m.auto.trig.consumption_conditions_hint')}</p>
  {/if}
</div>
