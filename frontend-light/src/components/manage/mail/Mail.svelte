<!-- Port of manage/mail/mail.component.html (Route mail). Host <pngx-mail> in App.svelte. -->
<script lang="ts">
  import '../../../styles/components/mail.css'
  import { t } from '../../../lib/i18n'
  import { icon } from '../../common/bootstrap-icon/icon'
  import { ricon } from '../../../lib/route-icons'
  import { Dropdown } from '../../../lib/dropdown.svelte'
  import { bodyContainer } from '../../../lib/body-container'
  import { modals } from '../../../lib/modal.svelte'
  import { router } from '../../../router.svelte'
  import { rawValue, session } from '../../../stores/settings.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { api } from '../../../api/client'
  import { deleteOf, listAllOf, patchOf } from '../../../api/admin'
  import PageHeader from '../../common/page-header/PageHeader.svelte'
  import ConfirmDialog from '../../common/confirm-dialog/ConfirmDialog.svelte'
  import PermissionsDialog from '../../common/permissions-dialog/PermissionsDialog.svelte'
  import MailAccountEditDialog from '../../common/edit-dialog/mail-account-edit-dialog/MailAccountEditDialog.svelte'
  import MailRuleEditDialog from '../../common/edit-dialog/mail-rule-edit-dialog/MailRuleEditDialog.svelte'
  import ProcessedMailDialog from './processed-mail-dialog/ProcessedMailDialog.svelte'
  import { MailAccountType, copyRule } from './mail-options'
  import './mail-icons'

  let mailAccounts = $state<any[] | undefined>([])
  let mailRules = $state<any[] | undefined>([])
  let loadingRules = $state(true)
  let showRules = $state(false)
  let loadingAccounts = $state(true)
  let showAccounts = $state(false)
  let oAuthAccountId: number | undefined
  const mailAccountsById = $derived(new Map((mailAccounts ?? []).map((a) => [a.id, a])))
  const gmailOAuthUrl = $derived(rawValue(session.settings, 'gmail_oauth_url') as string | undefined)
  const outlookOAuthUrl = $derived(rawValue(session.settings, 'outlook_oauth_url') as string | undefined)

  const reloadAccounts = () => listAllOf<any>('mail_accounts', { full_perms: true }).then((r) => (mailAccounts = r.results))
  const reloadRules = () => listAllOf<any>('mail_rules', { full_perms: true }).then((r) => (mailRules = r.results))

  reloadAccounts()
    .then(() => {
      loadingAccounts = false
      showAccounts = true
      if (oAuthAccountId) editMailAccount(mailAccounts?.find((a) => a.id === oAuthAccountId))
    })
    .catch((e) => {
      loadingAccounts = false
      toastService.showError(t('common.error_retrieving_mail_accounts'), e)
    })
  reloadRules()
    .then(() => {
      loadingRules = false
      showRules = true
    })
    .catch((e) => {
      loadingRules = false
      toastService.showError(t('manage.mail.error_retrieving_mail_rules'), e)
    })

  // queryParamMap: return from the OAuth flow
  {
    const q = new URLSearchParams(router.query)
    if (q.get('oauth_success')) {
      if (q.get('oauth_success') === '1') {
        toastService.showSuccess(t('manage.mail.oauth2_authentication_success'))
        oAuthAccountId = parseInt(q.get('account_id') ?? '')
        if ((mailAccounts?.length ?? 0) > 0) editMailAccount(mailAccounts?.find((a) => a.id === oAuthAccountId))
      } else toastService.showError(t('manage.mail.oauth2_authentication_failed_see'))
    }
  }

  const userCanEdit = (o: any) => session.hasObjectPermissions('change', o)
  const userIsOwner = (o: any) => session.ownsObject(o)

  function editMailAccount(account: any = null) {
    modals.open(
      MailAccountEditDialog,
      {
        dialogMode: account ? 'edit' : 'create',
        object: account,
        onsucceeded: (n: any) => {
          toastService.showSuccess(t('manage.mail.saved_account', { PH: n.name }))
          reloadAccounts()
        },
        onfailed: (e: any) => toastService.showError(t('manage.mail.error_saving_account'), e),
      },
      { backdrop: 'static', size: 'xl' },
    )
  }

  function confirmDelete(title: string, messageBold: string, run: () => Promise<void>, onError: (e: any) => void) {
    const ref = modals.open<string>(
      ConfirmDialog,
      {
        title,
        messageBold,
        message: t('common.operation_cannot_undone'),
        btnClass: 'btn-danger',
        btnCaption: t('common.proceed'),
        onconfirm: async (setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          try {
            await run()
            ref.close()
          } catch (e) {
            onError(e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  function deleteMailAccount(account: any) {
    confirmDelete(
      t('manage.mail.confirm_delete_mail_account'),
      t('manage.mail.operation_permanently_delete_mail'),
      async () => {
        await deleteOf('mail_accounts', account.id)
        toastService.showSuccess(t('manage.mail.deleted_mail_account', { PH: account.name }))
        reloadAccounts()
      },
      (e) => toastService.showError(t('manage.mail.error_deleting_mail_account', { PH: account.name }), e),
    )
  }

  async function processAccount(account: any) {
    try {
      await api.request('POST', `/api/mail_accounts/${account.id}/process/`, {})
      toastService.showInfo(t('manage.mail.processing_mail_account', { PH: account.name }))
    } catch (e) {
      toastService.showError(t('manage.mail.error_processing_mail_account', { PH: account.name }), e)
    }
  }

  function editMailRule(rule: any = null, forceCreate = false) {
    modals.open(
      MailRuleEditDialog,
      {
        dialogMode: rule && !forceCreate ? 'edit' : 'create',
        object: rule,
        onsucceeded: (n: any) => {
          toastService.showSuccess(t('manage.mail.saved_rule', { PH: n.name }))
          reloadRules()
        },
        onfailed: (e: any) => toastService.showError(t('manage.mail.error_saving_rule'), e),
      },
      { backdrop: 'static', size: 'xl' },
    )
  }

  async function onMailRuleEnableToggled(rule: any) {
    try {
      await patchOf('mail_rules', rule)
      toastService.showSuccess(rule.enabled ? t('manage.mail.rule_enabled', { PH: rule.name }) : t('manage.mail.rule_disabled', { PH: rule.name }))
    } catch (e) {
      toastService.showError(t('manage.mail.error_toggling_rule', { PH: rule.name }), e)
    }
  }

  function deleteMailRule(rule: any) {
    confirmDelete(
      t('manage.mail.confirm_delete_mail_rule'),
      t('manage.mail.operation_permanently_delete_mail_2'),
      async () => {
        await deleteOf('mail_rules', rule.id)
        toastService.showSuccess(t('manage.mail.deleted_mail_rule', { PH: rule.name }))
        reloadRules()
      },
      (e) => toastService.showError(t('manage.mail.error_deleting_mail_rule', { PH: rule.name }), e),
    )
  }

  function editPermissions(object: any) {
    const ref = modals.open(
      PermissionsDialog,
      {
        object,
        onconfirm: async ({ permissions }: any, setEnabled: (b: boolean) => void) => {
          setEnabled(false)
          object.owner = permissions['owner']
          object['set_permissions'] = permissions['set_permissions']
          try {
            await patchOf('account' in object ? 'mail_rules' : 'mail_accounts', object)
            toastService.showSuccess(t('common.permissions_updated'))
            ref.close()
          } catch (e) {
            toastService.showError(t('common.error_updating_permissions'), e)
          }
        },
      },
      { backdrop: 'static' },
    )
  }

  function viewProcessedMail(rule: any) {
    modals.open(ProcessedMailDialog, { rule }, { backdrop: 'static', size: 'xl' })
  }

  const menus = new Map<string, Dropdown>()
  const menu = (k: string) => {
    let d = menus.get(k)
    if (!d) menus.set(k, (d = new Dropdown()))
    return d
  }
</script>

<pngx-page-header>
<PageHeader
  title={t('manage.mail.mail_settings')}
  info={t('common.manage_e_mail_accounts')}
  infoLink="usage/#incoming-mail"
  >
</PageHeader>
</pngx-page-header>

{#if session.can('view', 'mailaccount')}
  <h4>
    {t('manage.mail.mail_accounts')}
    {#if session.can('add', 'mailaccount')}
      <button type="button" class="btn btn-sm btn-outline-primary ms-4" onclick={() => editMailAccount()}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('manage.mail.add_account')}
      </button>
    {/if}
    {#if gmailOAuthUrl}
      {#if session.can('add', 'mailaccount')}
        <a class="btn btn-sm btn-outline-secondary ms-2" href={gmailOAuthUrl}>
          <pngx-bs-icon name="google" class="me-1">{@html ricon('google')}</pngx-bs-icon>{t('manage.mail.connect_gmail_account')}
        </a>
      {/if}
    {/if}
    {#if outlookOAuthUrl}
      {#if session.can('add', 'mailaccount')}
        <a class="btn btn-sm btn-outline-secondary ms-2" href={outlookOAuthUrl}>
          <pngx-bs-icon name="microsoft" class="me-1">{@html ricon('microsoft')}</pngx-bs-icon>{t('manage.mail.connect_outlook_account')}
        </a>
      {/if}
    {/if}
  </h4>
  <ul class="list-group">
    <li class="list-group-item">
      <div class="row">
        <div class="col">{t('common.name')}</div>
        <div class="col">{t('manage.mail.server')}</div>
        <div class="col d-none d-sm-block">{t('common.username')}</div>
        <div class="col">{t('common.actions')}</div>
      </div>
    </li>

    {#if loadingAccounts}
      <li class="list-group-item">
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
      </li>
    {/if}

    {#each mailAccounts ?? [] as account (account)}
      {@const dd = menu('a' + account.id)}
      <li class="list-group-item">
        <div class="row fade" class:show={showAccounts}>
          <div class="col d-flex align-items-center">
            <button class="btn btn-link p-0 text-start" type="button" onclick={() => editMailAccount(account)} disabled={!session.can('change', 'mailaccount') || !userCanEdit(account)}>
              {account.name}{#if account.account_type === MailAccountType.IMAP}<pngx-bs-icon name="envelope-at-fill" class="ms-2">{@html ricon('envelope-at-fill')}</pngx-bs-icon>{:else if account.account_type === MailAccountType.Gmail_OAuth}<pngx-bs-icon name="google" class="ms-2">{@html ricon('google')}</pngx-bs-icon>{:else if account.account_type === MailAccountType.Outlook_OAuth}<pngx-bs-icon name="microsoft" class="ms-2">{@html ricon('microsoft')}</pngx-bs-icon>{/if}
            </button>
          </div>
          <div class="col d-flex align-items-center">{account.imap_server}</div>
          <div class="col d-flex align-items-center d-none d-sm-block">{account.username}</div>
          <div class="col">
            <div class="btn-group d-block d-sm-none">
              <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
                <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
                  <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
                </button>
                {#if dd.open}
                  <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:bodyContainer use:dd.menuEl>
                    {#if session.can('change', 'mailaccount')}
                      <button onclick={() => editMailAccount(account)} disabled={!userCanEdit(account)} class="dropdown-item">{t('common.edit')}</button>
                    {/if}
                    {#if userIsOwner(account)}
                      <button onclick={() => editPermissions(account)} class="dropdown-item">{t('common.permissions')}</button>
                    {/if}
                    {#if session.can('delete', 'mailaccount')}
                      <button onclick={() => deleteMailAccount(account)} disabled={!userIsOwner(account)} class="dropdown-item">{t('common.delete')}</button>
                    {/if}
                    {#if session.can('change', 'mailaccount')}
                      <button onclick={() => processAccount(account)} disabled={!userIsOwner(account)} class="dropdown-item">{t('manage.mail.process_mail')}</button>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
            <div class="btn-toolbar d-none d-sm-flex gap-2" role="toolbar">
              <div class="btn-group">
                {#if session.can('change', 'mailaccount')}
                  <button disabled={!userCanEdit(account)} class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editMailAccount(account)}>
                    <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                  </button>
                {/if}
                {#if userIsOwner(account)}
                  <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editPermissions(account)}>
                    <pngx-bs-icon width="1em" height="1em" name="person-lock" class="me-1">{@html ricon('person-lock', '1em', '1em')}</pngx-bs-icon>{t('common.permissions')}
                  </button>
                {/if}
                {#if session.can('delete', 'mailaccount')}
                  <button disabled={!userIsOwner(account)} class="btn btn-sm btn-outline-danger" type="button" onclick={() => deleteMailAccount(account)}>
                    <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                  </button>
                {/if}
              </div>
              <div class="btn-group">
                {#if session.can('change', 'mailaccount')}
                  <button disabled={!userIsOwner(account)} class="btn btn-sm btn-outline-secondary" type="button" onclick={() => processAccount(account)}>
                    <pngx-bs-icon width="1em" height="1em" name="arrow-clockwise" class="me-1">{@html ricon('arrow-clockwise', '1em', '1em')}</pngx-bs-icon>{t('manage.mail.process_mail')}
                  </button>
                {/if}
              </div>
            </div>
          </div>
        </div>
      </li>
    {/each}
    {#if !loadingAccounts && (mailAccounts?.length ?? 0) === 0}
      <li class="list-group-item">{t('manage.mail.no_mail_accounts_defined')}</li>
    {/if}
  </ul>
{/if}

{#if session.can('view', 'mailrule')}
  <h4 class="mt-4">
    {t('manage.mail.mail_rules')}
    {#if session.can('add', 'mailrule')}
      <button type="button" class="btn btn-sm btn-outline-primary ms-4" onclick={() => editMailRule()}>
        <pngx-bs-icon name="plus-circle" class="me-1">{@html icon('plus-circle')}</pngx-bs-icon>{t('manage.mail.add_rule')}
      </button>
    {/if}
  </h4>
  <ul class="list-group">
    <li class="list-group-item">
      <div class="row">
        <div class="col">{t('common.name')}</div>
        <div class="col-1 d-none d-sm-block">{t('common.sort_order')}</div>
        <div class="col-2">{t('common.account')}</div>
        <div class="col-2 d-none d-sm-block">{t('common.status')}</div>
        {#if session.can('view', 'processedmail')}
          <div class="col d-none d-sm-block">Processed Mail</div>
        {/if}
        <div class="col-3">{t('common.actions')}</div>
      </div>
    </li>

    {#if loadingRules}
      <li class="list-group-item">
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>{t('common.loading')}
      </li>
    {/if}

    {#each mailRules ?? [] as rule (rule)}
      {@const dd = menu('r' + rule.id)}
      <li class="list-group-item">
        <div class="row fade" class:show={showRules}>
          <div class="col d-flex align-items-center"><button class="btn btn-link p-0 text-start" type="button" onclick={() => editMailRule(rule)} disabled={!session.can('change', 'mailrule') || !userCanEdit(rule)}>{rule.name}</button></div>
          <div class="col-1 d-flex align-items-center d-none d-sm-flex">{rule.order}</div>
          <div class="col-2 d-flex align-items-center">{mailAccountsById.get(rule.account)?.name}</div>
          <div class="col-2 d-flex align-items-center d-none d-sm-flex">
            <div class="form-check form-switch mb-0">
              {#if session.can('change', 'mailrule')}
                <input type="checkbox" class="form-check-input cursor-pointer" id={rule.id + '_enable'} bind:checked={rule.enabled} onchange={() => onMailRuleEnableToggled(rule)}>
              {/if}
              <label class="form-check-label cursor-pointer" for={rule.id + '_enable'}>
                <code>{#if rule.enabled}{t('common.enabled')}{:else}<span class="text-muted">{t('common.disabled')}</span>{/if}</code>
              </label>
            </div>
          </div>
          {#if session.can('view', 'processedmail')}
            <div class="col d-flex align-items-center d-none d-sm-flex">
              <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => viewProcessedMail(rule)}>
                <pngx-bs-icon width="1em" height="1em" name="clock-history" class="me-1">{@html ricon('clock-history', '1em', '1em')}</pngx-bs-icon>{t('manage.mail.view_processed_mail')}
              </button>
            </div>
          {/if}
          <div class="col-3">
            <div class="btn-group d-block d-sm-none">
              <div class="d-inline-block {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
                <button type="button" class="btn btn-link dropdown-toggle" id="actionsMenuMobile" onclick={(e) => { e.stopPropagation(); dd.toggle(e) }} aria-label={t('common.actions')} aria-expanded={dd.open} use:dd.anchorEl>
                  <pngx-bs-icon name="three-dots-vertical">{@html icon('three-dots-vertical')}</pngx-bs-icon>
                </button>
                {#if dd.open}
                  <div aria-labelledby="actionsMenuMobile" class="dropdown-menu show" use:bodyContainer use:dd.menuEl>
                    {#if session.can('change', 'mailrule')}
                      <button onclick={() => editMailRule(rule)} disabled={!userCanEdit(rule)} class="dropdown-item">{t('common.edit')}</button>
                    {/if}
                    {#if userIsOwner(rule)}
                      <button onclick={() => editPermissions(rule)} class="dropdown-item">{t('common.permissions')}</button>
                    {/if}
                    {#if session.can('delete', 'mailrule')}
                      <button onclick={() => deleteMailRule(rule)} disabled={!userIsOwner(rule)} class="dropdown-item">{t('common.delete')}</button>
                    {/if}
                    {#if session.can('add', 'mailrule')}
                      <button onclick={() => editMailRule(copyRule(rule), true)} class="dropdown-item">{t('common.copy')}</button>
                    {/if}
                  </div>
                {/if}
              </div>
            </div>
            <div class="btn-toolbar d-none d-sm-flex gap-2" role="toolbar">
              <div class="btn-group">
                {#if session.can('change', 'mailrule')}
                  <button disabled={!userCanEdit(rule)} class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editMailRule(rule)}>
                    <pngx-bs-icon width="1em" height="1em" name="pencil" class="me-1">{@html icon('pencil', '1em', '1em')}</pngx-bs-icon>{t('common.edit')}
                  </button>
                {/if}
                {#if userIsOwner(rule)}
                  <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editPermissions(rule)}>
                    <pngx-bs-icon width="1em" height="1em" name="person-lock" class="me-1">{@html ricon('person-lock', '1em', '1em')}</pngx-bs-icon>{t('common.permissions')}
                  </button>
                {/if}
                {#if session.can('delete', 'mailrule')}
                  <button disabled={!userIsOwner(rule)} class="btn btn-sm btn-outline-danger" type="button" onclick={() => deleteMailRule(rule)}>
                    <pngx-bs-icon width="1em" height="1em" name="trash" class="me-1">{@html icon('trash', '1em', '1em')}</pngx-bs-icon>{t('common.delete')}
                  </button>
                {/if}
              </div>
              <div class="btn-group">
                {#if session.can('add', 'mailrule')}
                  <button class="btn btn-sm btn-outline-secondary" type="button" onclick={() => editMailRule(copyRule(rule), true)}>
                    <pngx-bs-icon width="1em" height="1em" name="files" class="me-1">{@html ricon('files', '1em', '1em')}</pngx-bs-icon>{t('common.copy')}
                  </button>
                {/if}
              </div>
            </div>
          </div>
        </div>
      </li>
    {/each}
    {#if !loadingRules && (mailRules?.length ?? 0) === 0}
      <li class="list-group-item">{t('manage.mail.no_mail_rules_defined')}</li>
    {/if}
  </ul>
{/if}

{#if !mailAccounts || !mailRules}
  <div>
    <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
    <div class="visually-hidden">{t('common.loading')}</div>
  </div>
{/if}
