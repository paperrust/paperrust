<!-- Email fetching (topic 10/11 B): accounts (management stays under Manage → Email) and flows = mail rule +
     the workflows that run on "Email arrives". Data: GET /rs/api/v2/automation/mail-flows, v1 /api/mail_accounts/. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { api } from '../../api/client'
  import Icon from '../components/Icon.svelte'
  import { listMailFlows, type MailFlowListItem } from './api'
  import { auto } from './store.svelte'

  let flows = $state<MailFlowListItem[] | null>(null)
  let failed = $state(false)
  const canAddRule = session.can('add', 'mailrule')
  const canManageAccounts = session.can('view', 'mailaccount')

  async function load() {
    failed = false
    try {
      const r = await listMailFlows()
      flows = Array.isArray(r) ? r : r.items
      await auto.load(true)
    } catch {
      failed = true
    }
  }
  onMount(load)
  const accountName = (id: number | null) => auto.mailAccounts.find((a) => a.id === id)?.name ?? (id == null ? '–' : `#${id}`)

  let fetching = $state<number | null>(null)
  async function fetchNow(id: number, name: string) {
    fetching = id
    try {
      await api.request('POST', `/api/mail_accounts/${id}/process/`)
      toastService.show({ content: t('manage.mail.processing_mail_account', { PH: name }), delay: 4000 })
    } catch (e) {
      toastService.showError(t('manage.mail.error_processing_mail_account', { PH: name }), e)
    } finally {
      fetching = null
    }
  }
</script>

<div class="au-page">
  <div class="au-head">
    <h1>{t('m.auto.mail_title')}</h1>
    {#if canAddRule}<a class="d2-btn primary" id="au-mail-new" href={router.href('automation/mail/new')}><Icon name="plus-lg" />{t('m.auto.mail.new_flow')}</a>{/if}
  </div>
  <div class="au-body d2-scroll">
    <p class="au-lead">{t('m.auto.mail.intro')}</p>
    {#if failed}
      <div class="au-banner danger" role="alert">{t('m.auto.load_failed')} <button type="button" class="d2-btn sm" onclick={load}>{t('m.auto.retry')}</button></div>
    {/if}

    <h2 class="au-section-title">{t('manage.mail.mail_accounts')}</h2>
    {#if !auto.mailAccounts.length}
      <div class="au-empty"><p>{t('manage.mail.no_mail_accounts_defined')}</p>{#if canManageAccounts}<a class="d2-btn" href={router.href('mail')}>{t('m.auto.mail.manage_accounts')}</a>{/if}</div>
    {:else}
      <div class="au-accounts" id="au-mail-accounts">
        {#each auto.mailAccounts as a (a.id)}
          <div class="au-account">
            <b>{a.name}</b>
            <span>{a.username ?? ''}{a.imap_server ? ` · ${a.imap_server}` : ''}</span>
            {#if session.can('change', 'mailaccount')}
              <div class="au-mt2"><button type="button" class="d2-btn sm" disabled={fetching === a.id} onclick={() => fetchNow(a.id, a.name)}><Icon name="arrow-clockwise" />{t('manage.mail.process_mail')}</button></div>
            {/if}
          </div>
        {/each}
      </div>
      {#if canManageAccounts}<p class="au-mt2"><a href={router.href('mail')}>{t('m.auto.mail.manage_accounts')}</a></p>{/if}
    {/if}

    <h2 class="au-section-title">{t('m.auto.mail.flows')}</h2>
    {#if flows === null && !failed}
      <p class="d2-muted" aria-busy="true">{t('common.loading')}</p>
    {:else if flows && !flows.length}
      <div class="au-empty"><p>{t('m.auto.mail.no_flows')}</p>{#if canAddRule}<a class="d2-btn primary" href={router.href('automation/mail/new')}><Icon name="plus-lg" />{t('m.auto.mail.new_flow')}</a>{/if}</div>
    {:else if flows}
      <ul class="au-list" id="au-mail-flows">
        {#each flows as f (f.mail_rule.id)}
          <li class="au-row" class:off={!f.mail_rule.enabled} data-cols="3">
            <Icon name="envelope" />
            <div class="main">
              <a class="name" href={router.href(`automation/mail/${f.mail_rule.id}`)}>{f.mail_rule.name}{#if !f.mail_rule.enabled}<span class="au-st skipped">{t('m.auto.off')}</span>{/if}</a>
              <p class="sent">{t('m.auto.mail.account_line', { PH: accountName(f.mail_rule.account) })}</p>
              <div class="au-chips">
                {#each f.workflows as w (w.id)}
                  <a class="au-chip" href={router.href(`automation/workflows/${w.id}`)}><Icon name="diagram-3" />{w.name}{#if !w.enabled} ({t('m.auto.off')}){/if}</a>
                {:else}
                  <span class="au-hint">{t('m.auto.mail.no_workflows')}</span>
                {/each}
              </div>
            </div>
            <a class="d2-btn sm" href={router.href(`automation/mail/${f.mail_rule.id}`)}>{t('common.edit')}</a>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>
