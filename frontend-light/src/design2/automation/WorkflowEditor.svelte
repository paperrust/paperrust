<!-- Workflow editor (topic 2 A + 3 B + 5 B): at the top the sentence with clickable parts, below cards from top to bottom
     "When → Only if → Then → Else", connected with lines. Clicking a card opens a side sheet on the right with
     all options of the part. Save: POST/PUT /rs/api/v2/automation/workflows (one transaction incl.
     else branch and AI condition); with trigger "Email arrives" and a changed rule via /mail-flows. -->
<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { t, locale } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { rawValue } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import Icon from '../components/Icon.svelte'
  import Menu from '../components/Menu.svelte'
  import SideSheet from '../components/SideSheet.svelte'
  import ConfirmDialog from './ConfirmDialog.svelte'
  import Sentence from './Sentence.svelte'
  import TriggerPanel from './panels/TriggerPanel.svelte'
  import ConditionsPanel from './panels/ConditionsPanel.svelte'
  import ActionPanel from './panels/ActionPanel.svelte'
  import AiPanel from './panels/AiPanel.svelte'
  import DryRunSheet from './DryRunSheet.svelte'
  import RunsSheet from './RunsSheet.svelte'
  import { allowedActionTypes, WORKFLOW_ACTION_OPTIONS } from '../../components/common/edit-dialog/workflow-edit-dialog/workflow-edit'
  import {
    createMailFlow,
    createWorkflow,
    deleteWorkflow,
    errorOf,
    getMailFlow,
    getWorkflow,
    listWorkflows,
    putMailFlow,
    putWorkflow,
    type CombinedWorkflow,
    type SaveBody,
  } from './api'
  import {
    COMMON_CONDITIONS,
    ADVANCED_CONDITIONS,
    WorkflowTriggerType,
    conditionAllowed,
    draftFromCombined,
    duplicateDraft,
    emptyDraft,
    errorTargets,
    fingerprint,
    fullAction,
    fullTrigger,
    isMailTrigger,
    mailTriggerIndex,
    moveItem,
    newMailRule,
    problems,
    quickAction,
    summarize,
    toSaveBody,
    type CondKey,
    type Draft,
    type QuickAction,
  } from './model'
  import { actionTexts, conditionText, sentenceConditions, triggerText, type Target } from './sentence'
  import { auto } from './store.svelte'

  let { id }: { id: string | null } = $props()
  const isNew = untrack(() => id === 'new' || id === null)
  const loc = locale()

  let draft = $state<Draft | null>(null)
  let saved = $state('')
  let note = $state<string | null>(null)
  let loadFailed = $state(false)
  let aiInfo = $state({ available: auto.aiOn, enabled: auto.aiOn })
  // Email trigger: rule object (new: id missing) and state at load time
  let mailRule = $state<Record<string, any> | null>(null)
  let mailSaved = $state('')

  let sheet = $state<{ target: Target; focus?: string | null } | null>(null)
  let side = $state<'dryrun' | 'runs' | null>(router.query.includes('tab=runs') ? 'runs' : null)
  let saving = $state(false)
  let saveProblems = $state<string[]>([])
  let errs = $state(errorTargets(null))
  let mailErrs = $state<Record<string, any>>({})
  let moreOpen = $state(false)
  let confirm = $state<'discard' | 'delete' | null>(null)

  const canChange = session.can(isNew ? 'add' : 'change', 'workflow')
  const canDelete = session.can('delete', 'workflow')
  const canAdd = session.can('add', 'workflow')

  function adopt(c: CombinedWorkflow) {
    draft = draftFromCombined(c)
    aiInfo = c.ai
    saved = fingerprint(draft)
  }
  async function loadRule(ruleId: number | null) {
    if (ruleId === null) {
      mailRule = null
      mailSaved = ''
      return
    }
    if (ruleId < 0) {
      mailRule = newMailRule(auto.mailAccounts[0]?.id ?? null)
      mailRule.name = draft?.workflow.name ?? ''
      mailSaved = ''
      return
    }
    try {
      const r = (await getMailFlow(ruleId)).mail_rule
      mailRule = r
      mailSaved = JSON.stringify(r)
    } catch {
      const r = auto.mailRules.find((x) => x.id === ruleId)
      mailRule = r ? { ...r } : null
      mailSaved = JSON.stringify(mailRule)
    }
  }

  onMount(() => {
    void (async () => {
      await auto.load()
      if (isNew) {
        const p = auto.pending
        auto.pending = null
        draft = p?.draft ?? emptyDraft('')
        note = p?.note ?? null
        saved = ''
        // new workflow at the end of the list (order = execution order)
        if (!draft.workflow.order)
          try {
            const items = (await listWorkflows()).items
            draft.workflow.order = items.reduce((m, x) => Math.max(m, x.order), 0) + 10
          } catch {
            // without a list the default 0 stays
          }
      } else {
        try {
          adopt(await getWorkflow(Number(id)))
        } catch {
          loadFailed = true
          return
        }
      }
      const mi = draft ? mailTriggerIndex(draft.workflow) : -1
      if (draft && mi >= 0) await loadRule(draft.workflow.triggers[mi].filter_mailrule)
      // New workflow: open the trigger sheet if nothing is chosen yet (empty template)
      if (isNew && draft && !draft.workflow.actions.length) sheet = { target: { card: 'action', index: -1 } }
    })()
    const beforeunload = (e: BeforeUnloadEvent) => {
      if (dirty) e.preventDefault()
    }
    addEventListener('beforeunload', beforeunload)
    return () => removeEventListener('beforeunload', beforeunload)
  })

  async function retryLoad() {
    loadFailed = false
    try {
      adopt(await getWorkflow(Number(id)))
      const mi = draft ? mailTriggerIndex(draft.workflow) : -1
      if (draft && mi >= 0) await loadRule(draft.workflow.triggers[mi].filter_mailrule)
    } catch {
      loadFailed = true
    }
  }

  const mailDirty = $derived(!!mailRule && (mailRule.id == null || JSON.stringify(mailRule) !== mailSaved))
  const dirty = $derived(!!draft && (fingerprint(draft) !== saved || mailDirty))
  const summary = $derived(draft ? summarize(draft, auto.names, aiInfo) : null)

  // ---- Selection for "+ Condition" / "+ Action" ------------------------------------------------------------
  const COND_LABEL: Record<CondKey, string> = {
    text: 'm.auto.cond.text',
    tags: 'm.auto.cond.tags',
    correspondent: 'm.auto.cond.correspondent',
    document_type: 'm.auto.cond.document_type',
    storage_path: 'm.auto.cond.storage_path',
    filename: 'm.auto.cond.filename',
    path: 'm.auto.cond.path',
    mail_rule: 'm.auto.cond.mail_rule',
    sources: 'm.auto.cond.sources',
    custom_field_query: 'm.auto.cond.custom_field_query',
    ai: 'm.auto.cond.ai',
  }
  const firstType = $derived(draft?.workflow.triggers[0]?.type ?? WorkflowTriggerType.DocumentAdded)
  const condOptions = $derived(
    draft?.workflow.triggers.length
      ? [
          ...[...COMMON_CONDITIONS, ...ADVANCED_CONDITIONS].filter((k) => conditionAllowed(k, firstType) && !(isMailTrigger(draft!.workflow.triggers[0]) && (k === 'mail_rule' || k === 'sources'))).map((k) => ({ key: k, label: t(COND_LABEL[k]) })),
          ...(auto.aiOn && !draft.ai_condition && draft.workflow.triggers.some((x) => x.type !== WorkflowTriggerType.Consumption) ? [{ key: 'ai' as CondKey, label: t('m.auto.cond.ai') }] : []),
        ]
      : [],
  )
  const quickActions = $derived(
    [
      { key: 'tags' as QuickAction, label: t('m.auto.quick.tags'), icon: 'tags' },
      { key: 'correspondent' as QuickAction, label: t('m.auto.quick.correspondent'), icon: 'person' },
      { key: 'document_type' as QuickAction, label: t('m.auto.quick.document_type'), icon: 'file-earmark-text' },
      { key: 'storage_path' as QuickAction, label: t('m.auto.quick.storage_path'), icon: 'folder' },
      { key: 'title' as QuickAction, label: t('m.auto.quick.title'), icon: 'pencil' },
      ...(auto.emailOn ? [{ key: 'email' as QuickAction, label: t('m.auto.quick.email'), icon: 'envelope' }] : []),
      ...(auto.aiOn ? [{ key: 'ai' as QuickAction, label: t('m.auto.quick.ai'), icon: 'stars' }] : []),
      { key: 'trash' as QuickAction, label: t('m.auto.quick.trash'), icon: 'trash' },
      { key: 'other' as QuickAction, label: t('m.auto.quick.other'), icon: 'three-dots' },
    ],
  )
  const actionTypes = $derived.by(() => {
    if (!draft) return [] as { id: number; name: string }[]
    const allowed = allowedActionTypes(draft.workflow as any, {
      emailEnabled: auto.emailOn,
      remoteOcrConfigured: !!rawValue(session.settings, 'remote_ocr:configured'),
      aiEnabled: auto.aiOn,
    })
    return allowed
  })
  /** allowed kinds + the one already chosen (never hide an option that is saved) */
  const typesFor = (current: number) => (actionTypes.some((x) => x.id === current) ? actionTypes : [...actionTypes, ...WORKFLOW_ACTION_OPTIONS.filter((x) => x.id === current)])

  // ---- Editing ---------------------------------------------------------------------------------------
  function open(target: Target, focus: string | null = null) {
    sheet = { target, focus }
  }
  function addCondition(k: CondKey) {
    if (!draft) return
    if (k === 'ai') {
      draft.ai_condition = { field: 'document_type', object_id: 0, min_confidence: 0.9 }
      open({ card: 'ai' })
      return
    }
    open({ card: 'conditions', index: 0 }, k)
  }
  function addAction(k: QuickAction, branch: 'action' | 'else' = 'action') {
    if (!draft) return
    const q = quickAction(k)
    const list = branch === 'action' ? draft.workflow.actions : draft.else_actions
    list.push(q.action)
    open({ card: branch, index: list.length - 1 }, q.focus)
  }
  function addTrigger() {
    if (!draft) return
    draft.workflow.triggers.push(fullTrigger({ type: WorkflowTriggerType.DocumentUpdated }))
    open({ card: 'trigger', index: draft.workflow.triggers.length - 1 })
  }
  function removeTarget(tg: Target) {
    if (!draft) return
    if (tg.card === 'trigger') {
      const wasMail = isMailTrigger(draft.workflow.triggers[tg.index] ?? {})
      draft.workflow.triggers.splice(tg.index, 1)
      if (wasMail && mailTriggerIndex(draft.workflow) < 0) void loadRule(null)
    } else if (tg.card === 'action') draft.workflow.actions.splice(tg.index, 1)
    else if (tg.card === 'else') draft.else_actions.splice(tg.index, 1)
    else if (tg.card === 'ai') draft.ai_condition = null
    else if (tg.card === 'conditions') {
      const tr = draft.workflow.triggers[tg.index]
      const keep = fullTrigger({ id: tr.id, type: tr.type, sources: isMailTrigger(tr) ? tr.sources : [], filter_mailrule: isMailTrigger(tr) ? tr.filter_mailrule : null, ...scheduleOf(tr) })
      draft.workflow.triggers[tg.index] = keep
    }
    sheet = null
  }
  const scheduleOf = (tr: Record<string, any>) => ({
    schedule_offset_days: tr.schedule_offset_days,
    schedule_is_recurring: tr.schedule_is_recurring,
    schedule_recurring_interval_days: tr.schedule_recurring_interval_days,
    schedule_date_field: tr.schedule_date_field,
    schedule_date_custom_field: tr.schedule_date_custom_field,
  })
  function move(branch: 'action' | 'else', from: number, to: number) {
    if (!draft) return
    if (branch === 'action') draft.workflow.actions = moveItem(draft.workflow.actions, from, to)
    else draft.else_actions = moveItem(draft.else_actions, from, to)
  }
  // Dragging the action cards
  let drag = $state<{ branch: 'action' | 'else'; from: number } | null>(null)
  let dropAt = $state<{ branch: 'action' | 'else'; index: number; after: boolean } | null>(null)
  function dragOver(e: DragEvent, branch: 'action' | 'else', i: number) {
    if (!drag || drag.branch !== branch) return
    e.preventDefault()
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
    dropAt = { branch, index: i, after: e.clientY > r.top + r.height / 2 }
  }
  function drop(e: DragEvent) {
    e.preventDefault()
    if (!drag || !dropAt || drag.branch !== dropAt.branch) return
    let to = dropAt.index + (dropAt.after ? 1 : 0)
    if (to > drag.from) to--
    move(drag.branch, drag.from, to)
    drag = null
    dropAt = null
  }

  // ---- Saving ----------------------------------------------------------------------------------------
  const PROBLEM: Record<string, string> = {
    no_name: 'm.auto.problem.no_name',
    no_trigger: 'm.auto.problem.no_trigger',
    no_action: 'm.auto.problem.no_action',
    ai_condition_incomplete: 'm.auto.problem.ai_incomplete',
  }
  function bodyFor(d: Draft): SaveBody {
    const b = toSaveBody(d)
    // new rule: do not send placeholder -1 (the server binds the trigger to the new rule)
    for (const tr of b.triggers) if (tr.filter_mailrule != null && tr.filter_mailrule < 0) tr.filter_mailrule = null
    return b
  }
  async function save(): Promise<boolean> {
    if (!draft || saving) return false
    saveProblems = problems(draft).map((p) => t(PROBLEM[p]))
    if (mailRule && mailTriggerIndex(draft.workflow) >= 0 && !mailRule.account) saveProblems.push(t('m.auto.problem.no_account'))
    if (saveProblems.length) return false
    saving = true
    errs = errorTargets(null)
    mailErrs = {}
    try {
      const body = bodyFor(draft)
      const mi = mailTriggerIndex(draft.workflow)
      let result: CombinedWorkflow
      if (mi >= 0 && mailRule && mailDirty) {
        const rule = { ...mailRule }
        if (!rule.name) rule.name = draft.workflow.name
        const r = rule.id != null ? await putMailFlow(rule.id, { mail_rule: rule, workflows: [body] }) : await createMailFlow({ mail_rule: rule, workflows: [body] })
        const mine = r.workflows.find((w) => w.workflow.id === body.id) ?? r.workflows.reduce((a, b) => ((b.workflow.id ?? 0) > (a.workflow.id ?? 0) ? b : a))
        result = mine
        mailRule = r.mail_rule
        mailSaved = JSON.stringify(r.mail_rule)
        void auto.load(true)
      } else {
        result = draft.workflow.id != null ? await putWorkflow(draft.workflow.id, body) : await createWorkflow(body)
      }
      adopt(result)
      toastService.show({ content: t('m.auto.saved', { PH: result.workflow.name }), delay: 3000, level: 'success' })
      if (isNew && result.workflow.id != null) router.navigate(`automation/workflows/${result.workflow.id}`, true)
      return true
    } catch (e) {
      const err = errorOf(e)
      if (err.code === 'invalid' && err.fields) {
        const fields: Record<string, any> = { ...err.fields }
        for (const [k, v] of Object.entries(err.fields)) if (/^workflows\.\d+$/.test(k) && v && typeof v === 'object') Object.assign(fields, v), delete fields[k]
        errs = errorTargets(fields)
        mailErrs = typeof fields.mail_rule === 'object' && !Array.isArray(fields.mail_rule) ? fields.mail_rule : {}
        saveProblems = [t('m.auto.fix_marked')]
      } else saveProblems = [err.status === 403 ? t('m.auto.not_allowed') : err.message || t('m.auto.could_not_save')]
      return false
    } finally {
      saving = false
    }
  }

  async function duplicate() {
    moreOpen = false
    if (!draft) return
    try {
      const d = duplicateDraft(draft, t('m.auto.copy_name', { PH: draft.workflow.name ?? '' }))
      const c = await createWorkflow(bodyFor(d))
      toastService.show({ content: t('m.auto.duplicated', { PH: c.workflow.name }), delay: 4000 })
      router.navigate(`automation/workflows/${c.workflow.id}`)
    } catch (e) {
      toastService.showError(t('m.auto.could_not_save'), e)
    }
  }
  async function remove() {
    if (!draft?.workflow.id) return
    try {
      await deleteWorkflow(draft.workflow.id)
      toastService.show({ content: t('m.auto.deleted', { PH: draft.workflow.name }), delay: 4000 })
      saved = fingerprint(draft)
      mailSaved = JSON.stringify(mailRule)
      router.navigate('automation/workflows')
    } catch (e) {
      toastService.showError(t('m.auto.could_not_delete'), e)
    } finally {
      confirm = null
    }
  }
  function back() {
    if (dirty) confirm = 'discard'
    else router.navigate('automation/workflows')
  }
  function onkeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      e.preventDefault()
      void save()
    }
  }

  // ---- Display of the cards -------------------------------------------------------------------------------
  const TRIG_LABEL: Record<string, string> = {
    consumption: 'm.auto.trig.consumption',
    document_added: 'm.auto.trig.added',
    document_updated: 'm.auto.trig.updated',
    scheduled: 'm.auto.trig.scheduled',
    manual: 'm.auto.trig.manual',
  }
  const ACT_LABEL: Record<number, string> = {
    1: 'dialog.workflow.assignment',
    2: 'dialog.workflow.removal',
    3: 'common.email',
    4: 'dialog.workflow.webhook',
    5: 'dialog.workflow.password_removal',
    6: 'common.move_trash',
    7: 'common.remote_ocr',
    8: 'm.auto.act.ai',
  }
  const sheetTitle = $derived.by(() => {
    const tg = sheet?.target
    if (!tg) return ''
    if (tg.card === 'trigger') return t('m.auto.card.when')
    if (tg.card === 'conditions') return t('m.auto.card.only_if')
    if (tg.card === 'ai') return t('m.auto.card.ai')
    if (tg.card === 'else') return t('m.auto.card.else')
    return t('m.auto.card.then')
  })
  /** Action of the side sheet; index -1 = no action yet (choose new action) */
  const sheetAction = $derived.by(() => {
    const tg = sheet?.target
    if (!draft || !tg || (tg.card !== 'action' && tg.card !== 'else')) return null
    const list = tg.card === 'action' ? draft.workflow.actions : draft.else_actions
    return list[tg.index] ?? null
  })
  $effect(() => {
    // Close the sheet of a deleted/missing card (except "choose new action" / "no trigger yet")
    const tg = sheet?.target
    if (!draft || !tg) return
    if ((tg.card === 'action' || tg.card === 'else') && tg.index >= 0 && !sheetAction) sheet = null
    if (tg.card === 'ai' && !draft.ai_condition) sheet = null
  })
  function ensureTrigger() {
    if (draft && !draft.workflow.triggers.length) draft.workflow.triggers.push(fullTrigger({ type: WorkflowTriggerType.DocumentAdded }))
  }
  $effect(() => {
    if (sheet?.target.card === 'trigger' || sheet?.target.card === 'conditions') untrack(ensureTrigger)
  })
  const isSel = (tg: Target) => !!sheet && sheet.target.card === tg.card && ('index' in tg ? tg.index : -1) === ('index' in sheet.target ? sheet.target.index : -1)
  const mailRuleName = (tr: Record<string, any>) => (tr.filter_mailrule != null && tr.filter_mailrule < 0 ? mailRule?.name || t('m.auto.trig.new_rule') : null)
</script>

<svelte:window {onkeydown} />

{#snippet actionCard(a: Record<string, any>, i: number, branch: 'action' | 'else', count: number)}
  {@const s = summary?.[branch === 'action' ? 'actions' : 'else_actions'][i]}
  {@const er = (branch === 'action' ? errs.actions : errs.else)[i]}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="au-card {branch === 'action' ? 'do' : 'else'}"
    class:sel={isSel({ card: branch, index: i })}
    class:has-error={!!er}
    class:dragging={drag?.branch === branch && drag.from === i}
    class:drop-before={dropAt?.branch === branch && dropAt.index === i && !dropAt.after}
    class:drop-after={dropAt?.branch === branch && dropAt.index === i && dropAt.after}
    ondragover={(e) => dragOver(e, branch, i)}
    ondrop={drop}
    data-card="{branch}-{i}"
  >
    {#if canChange && count > 1}
      <button type="button" class="au-grip" draggable="true" aria-label={t('m.auto.drag_action')} title={t('m.auto.drag_action')} ondragstart={(e) => ((drag = { branch, from: i }), e.dataTransfer?.setData('text/plain', String(i)))} ondragend={() => ((drag = null), (dropAt = null))} onkeydown={(e) => {
        if (e.altKey && e.key === 'ArrowUp' && i > 0) (e.preventDefault(), move(branch, i, i - 1))
        if (e.altKey && e.key === 'ArrowDown' && i < count - 1) (e.preventDefault(), move(branch, i, i + 1))
      }}><Icon name="grip-vertical" /></button>
    {/if}
    <button type="button" class="open" id="au-card-{branch}-{i}" onclick={() => open({ card: branch, index: i })}>
      <span class="k">{i + 1}. {t(ACT_LABEL[a.type] ?? 'm.auto.act.unknown')}</span>
      <span class="v">{s ? actionTexts(s, t, loc).join(' · ') : ''}</span>
      {#if er}<span class="err">{er.join(' ')}</span>{/if}
    </button>
    {#if canChange && count > 1}
      <span class="tools">
        <button type="button" class="d2-btn ghost icon sm" aria-label={t('m.auto.move_up')} disabled={i === 0} onclick={() => move(branch, i, i - 1)}><Icon name="arrow-up" /></button>
        <button type="button" class="d2-btn ghost icon sm" aria-label={t('m.auto.move_down')} disabled={i === count - 1} onclick={() => move(branch, i, i + 1)}><Icon name="arrow-down" /></button>
      </span>
    {/if}
  </div>
{/snippet}

<div class="au-page" id="au-editor">
  <div class="au-editor-head">
    <button type="button" class="d2-btn ghost icon" aria-label={t('m.auto.back')} title={t('m.auto.back')} onclick={back}><Icon name="arrow-left" /></button>
    {#if draft}
      <label class="d2-sr" for="au-name">{t('common.name')}</label>
      <input id="au-name" class="au-name" placeholder={t('m.auto.name_placeholder')} bind:value={draft.workflow.name} disabled={!canChange} />
      <button type="button" class="au-switch" role="switch" id="au-enabled" aria-checked={draft.workflow.enabled} aria-label={t('m.auto.active')} title={t('m.auto.active')} disabled={!canChange} onclick={() => draft && (draft.workflow.enabled = !draft.workflow.enabled)}></button>
      <span class="au-save-state" aria-live="polite">{saving ? t('m.auto.saving') : dirty ? t('m.auto.unsaved') : t('m.auto.all_saved')}</span>
      <button type="button" class="d2-btn" id="au-open-dryrun" onclick={() => (side = 'dryrun')}><Icon name="play" />{t('m.auto.dryrun')}</button>
      {#if draft.workflow.id != null}<button type="button" class="d2-btn ghost" id="au-open-runs" onclick={() => (side = 'runs')}><Icon name="clock-history" />{t('m.auto.history')}</button>{/if}
      <div class="au-adder">
        <button type="button" class="d2-btn ghost icon" aria-label={t('m.auto.more')} aria-haspopup="menu" aria-expanded={moreOpen} onclick={() => (moreOpen = !moreOpen)}><Icon name="three-dots" /></button>
        {#if moreOpen}
          <Menu label={t('m.auto.more')} onclose={() => (moreOpen = false)}>
            {#if canAdd && draft.workflow.id != null}<button type="button" class="d2-menu-item" role="menuitem" id="au-duplicate" onclick={duplicate}><Icon name="copy" />{t('m.auto.duplicate')}</button>{/if}
            {#if canDelete && draft.workflow.id != null}<button type="button" class="d2-menu-item" role="menuitem" id="au-delete" onclick={() => ((moreOpen = false), (confirm = 'delete'))}><Icon name="trash" />{t('common.delete')}</button>{/if}
            {#if draft.workflow.id == null}<span class="d2-menu-label">{t('m.auto.save_first')}</span>{/if}
          </Menu>
        {/if}
      </div>
      {#if canChange}<button type="button" class="d2-btn primary" id="au-save" disabled={saving || (!dirty && !isNew)} onclick={save}>{t('common.save')}</button>{/if}
    {/if}
  </div>

  <div class="au-editor-body d2-scroll">
    <div class="au-editor-inner">
      {#if loadFailed}
        <div class="au-banner danger" role="alert">{t('m.auto.load_failed')} <button type="button" class="d2-btn sm" onclick={retryLoad}>{t('m.auto.retry')}</button></div>
      {:else if !draft || !summary}
        <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
      {:else}
        {#if note}<div class="au-banner" role="status"><Icon name="stars" /> {note}</div>{/if}
        {#if isNew}<div class="au-banner" role="status">{t('m.auto.new_hint')}</div>{/if}
        {#if saveProblems.length}<div class="au-banner danger" role="alert" id="au-problems"><ul>{#each saveProblems as p, i (i)}<li>{p}</li>{/each}</ul></div>{/if}
        {#if errs.general.length}<div class="au-banner danger" role="alert"><ul>{#each errs.general as p, i (i)}<li>{p}</li>{/each}</ul></div>{/if}

        <Sentence
          {summary}
          active={sheet?.target ?? null}
          conditions={condOptions}
          {quickActions}
          canElse={canChange}
          onopen={(tg) => open(tg)}
          onaddcondition={addCondition}
          onaddaction={(k) => addAction(k)}
          onaddelse={() => addAction('other', 'else')}
        />

        <div class="au-flow" id="au-flow">
          <h2 class="au-step-label"><Icon name="lightning" />{t('m.auto.card.when')}</h2>
          {#each draft.workflow.triggers as tr, i (i)}
            {@const st = summary.triggers[i]}
            {#if i > 0}<div class="au-or">{t('m.auto.or')}</div>{/if}
            <div class="au-card when" class:sel={isSel({ card: 'trigger', index: i })} class:has-error={!!errs.triggers[i]}>
              <button type="button" class="open" id="au-card-trigger-{i}" onclick={() => open({ card: 'trigger', index: i })}>
                <span class="k">{t(isMailTrigger(tr) ? 'm.auto.trig.mail' : (TRIG_LABEL[st?.type_key ?? ''] ?? 'm.auto.trig.added'))}</span>
                <span class="v">{mailRuleName(tr) ? t('m.auto.s.t_mail', { PH: mailRuleName(tr) }) : st ? triggerText(st, t) : ''}</span>
                {#if errs.triggers[i]}<span class="err">{errs.triggers[i].join(' ')}</span>{/if}
              </button>
            </div>
            <div class="au-link" aria-hidden="true"></div>
            <div class="au-card if" class:sel={isSel({ card: 'conditions', index: i })}>
              <button type="button" class="open" id="au-card-conditions-{i}" onclick={() => open({ card: 'conditions', index: i })}>
                <span class="k">{t('m.auto.card.only_if')}</span>
                {#if st && sentenceConditions(st).length}
                  <span class="au-chips">{#each sentenceConditions(st) as c, ci (ci)}<span class="au-chip">{conditionText(c, t, loc)}</span>{/each}</span>
                {:else}
                  <span class="sub">{t('m.auto.card.always')}</span>
                {/if}
              </button>
            </div>
          {/each}
          {#if !draft.workflow.triggers.length}
            <div class="au-card when"><button type="button" class="open" id="au-card-trigger-0" onclick={() => open({ card: 'trigger', index: 0 })}><span class="k">{t('m.auto.card.when')}</span><span class="sub">{t('m.auto.s.choose_trigger')}</span></button></div>
          {/if}
          {#if draft.ai_condition}
            <div class="au-link" aria-hidden="true"></div>
            <div class="au-card if" class:sel={isSel({ card: 'ai' })} class:has-error={!!errs.ai.length}>
              <button type="button" class="open" id="au-card-ai" onclick={() => open({ card: 'ai' })}>
                <span class="k"><Icon name="stars" /> {t('m.auto.card.ai')}</span>
                <span class="v">{summary.ai_condition ? t(({ document_type: 'm.auto.s.c_ai_document_type', correspondent: 'm.auto.s.c_ai_correspondent', storage_path: 'm.auto.s.c_ai_storage_path', tags: 'm.auto.s.c_ai_tags' } as Record<string, string>)[summary.ai_condition.field], { PH: t('m.auto.s.quote', { PH: summary.ai_condition.object.name ?? '…' }), PCT: Math.round(summary.ai_condition.min_confidence * 100) }) : ''}</span>
                {#if summary.ai_condition && !summary.ai_condition.active}<span class="sub"><span class="au-st warn">{t('m.auto.ai.inactive')}</span></span>{/if}
                {#if errs.ai.length}<span class="err">{errs.ai.join(' ')}</span>{/if}
              </button>
            </div>
          {/if}
          {#if canChange}
            <div class="au-add-row">
              <button type="button" class="d2-btn sm ghost" id="au-add-trigger" onclick={addTrigger}><Icon name="plus-lg" />{t('m.auto.add_trigger')}</button>
            </div>
          {/if}

          <div class="au-link" aria-hidden="true"></div>
          <h2 class="au-step-label"><Icon name="arrow-return-right" />{t('m.auto.card.then')}</h2>
          {#each draft.workflow.actions as a, i (a)}
            {@render actionCard(a, i, 'action', draft.workflow.actions.length)}
          {/each}
          {#if !draft.workflow.actions.length}<p class="au-hint">{t('m.auto.no_actions')}</p>{/if}
          {#if errs.actions[-1]}<p class="au-err">{errs.actions[-1].join(' ')}</p>{/if}
          {#if canChange}
            <div class="au-add-row"><button type="button" class="d2-btn sm" id="au-add-action-card" onclick={() => addAction('other')}>{t('m.auto.add_action')}</button></div>
          {/if}

          <div class="au-link" aria-hidden="true"></div>
          <h2 class="au-step-label"><Icon name="signpost-split" />{t('m.auto.card.else')}</h2>
          {#if draft.else_actions.length}
            <p class="au-hint">{t('m.auto.else_hint')}</p>
            {#each draft.else_actions as a, i (a)}
              {@render actionCard(a, i, 'else', draft.else_actions.length)}
            {/each}
          {:else}
            <p class="au-hint">{t('m.auto.else_empty')}</p>
          {/if}
          {#if canChange}
            <div class="au-add-row"><button type="button" class="d2-btn sm ghost" id="au-add-else-card" onclick={() => addAction('other', 'else')}><Icon name="plus-lg" />{t('m.auto.add_else_action')}</button></div>
          {/if}
        </div>
      {/if}
    </div>
  </div>
</div>

{#if sheet && draft}
  {@const tg = sheet.target}
  <SideSheet title={sheetTitle} onclose={() => (sheet = null)} width="520px">
    {#if tg.card === 'trigger' && draft.workflow.triggers[tg.index]}
      <TriggerPanel trigger={draft.workflow.triggers[tg.index]} index={tg.index} errors={errs.triggers[tg.index] ?? []} {mailRule} mailErrors={mailErrs} onpickrule={(r) => loadRule(r)} />
    {:else if tg.card === 'conditions' && draft.workflow.triggers[tg.index]}
      {#key tg.index}
        <ConditionsPanel trigger={draft.workflow.triggers[tg.index]} index={tg.index} focus={(sheet.focus as CondKey | null) ?? null} errors={errs.triggers[tg.index] ?? []} />
      {/key}
    {:else if tg.card === 'ai' && draft.ai_condition}
      <AiPanel cond={draft.ai_condition} reason={summary?.ai_condition?.inactive_reason ?? null} errors={errs.ai} />
    {:else if (tg.card === 'action' || tg.card === 'else') && sheetAction}
      {#key `${tg.card}-${tg.index}`}
        <ActionPanel action={sheetAction} index={tg.index} branch={tg.card === 'action' ? 'main' : 'else'} types={typesFor(sheetAction.type)} focus={sheet.focus ?? null} errors={(tg.card === 'action' ? errs.actions : errs.else)[tg.index] ?? []} />
      {/key}
    {:else if tg.card === 'action' || tg.card === 'else'}
      <!-- no action yet: quick pick -->
      <div class="au-form">
        <p class="au-hint">{t('m.auto.pick_action')}</p>
        <div class="au-tiles">
          {#each quickActions as q (q.key)}
            <button type="button" class="au-tile" id="au-pick-{q.key}" onclick={() => addAction(q.key, tg.card === 'else' ? 'else' : 'action')}><b><Icon name={q.icon} />{q.label}</b></button>
          {/each}
        </div>
      </div>
    {/if}
    {#snippet footer()}
      {#if canChange && !((tg.card === 'action' || tg.card === 'else') && !sheetAction)}
        <button type="button" class="d2-btn ghost au-push" id="au-sheet-remove" onclick={() => removeTarget(tg)}><Icon name="trash" />{tg.card === 'conditions' ? t('m.auto.clear_conditions') : t('m.auto.remove')}</button>
      {/if}
      <button type="button" class="d2-btn primary" id="au-sheet-done" onclick={() => (sheet = null)}>{t('m.auto.done')}</button>
    {/snippet}
  </SideSheet>
{/if}

{#if side === 'dryrun' && draft}
  <DryRunSheet body={bodyFor(draft)} workflowId={draft.workflow.id ?? null} {dirty} triggers={draft.workflow.triggers.map((x) => x.type)} hasAi={!!draft.ai_condition} aiOn={aiInfo.available && aiInfo.enabled} onclose={() => (side = null)} onsave={save} />
{/if}
{#if side === 'runs' && draft?.workflow.id != null}
  <RunsSheet workflowId={draft.workflow.id} onclose={() => (side = null)} />
{/if}
{#if confirm === 'discard'}
  <ConfirmDialog title={t('m.auto.discard_title')} confirmLabel={t('m.auto.discard')} danger onconfirm={() => {
    confirm = null
    saved = draft ? fingerprint(draft) : ''
    mailSaved = JSON.stringify(mailRule)
    router.navigate('automation/workflows')
  }} oncancel={() => (confirm = null)}>
    <p>{t('m.auto.discard_text')}</p>
  </ConfirmDialog>
{/if}
{#if confirm === 'delete' && draft}
  <ConfirmDialog title={t('m.auto.delete_title', { PH: draft.workflow.name ?? '' })} confirmLabel={t('common.delete')} danger onconfirm={remove} oncancel={() => (confirm = null)}>
    <p>{t('m.auto.delete_text')}</p>
  </ConfirmDialog>
{/if}
