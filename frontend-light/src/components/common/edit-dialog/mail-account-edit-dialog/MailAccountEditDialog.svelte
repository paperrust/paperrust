<!-- Port of common/edit-dialog/mail-account-edit-dialog/mail-account-edit-dialog.component.html (size xl). -->
<script lang="ts">
  import '../../../../styles/components/mail-account-edit-dialog.css'
  import '../../../../styles/parts/alert.css'
  import { t } from '../../../../lib/i18n'
  import { api } from '../../../../api/client'
  import { EditDialogState, type EditDialogProps } from '../edit-dialog.svelte'
  import InputText from '../../input/text/InputText.svelte'
  import InputPassword from '../../input/password/InputPassword.svelte'
  import InputSelect from '../../input/select/InputSelect.svelte'
  import InputCheck from '../../input/check/InputCheck.svelte'
  import { IMAP_SECURITY_OPTIONS, IMAPSecurity, translated } from '../../../manage/mail/mail-options'

  let props: EditDialogProps<any> = $props()
  const dlg = new EditDialogState<any>('mail_accounts' as any, props, {
    name: null,
    imap_server: null,
    imap_port: null,
    imap_security: IMAPSecurity.SSL,
    username: null,
    password: null,
    is_token: false,
    character_set: 'UTF-8',
  })
  const title = $derived(dlg.mode === 'create' ? t('dialog.mail_account.create_new_mail_account') : t('dialog.mail_account.edit_mail_account'))
  const imapSecurityOptions = translated(IMAP_SECURITY_OPTIONS, t)
  let testActive = $state(false)
  let testResult = $state<string | null>(null)
  let alertTimeout: ReturnType<typeof setTimeout> | undefined
  const testResultMessage = $derived(testResult === 'success' ? t('dialog.mail_account.successfully_connected_mail_server') : t('dialog.mail_account.unable_connect_mail_server'))

  async function test() {
    testActive = true
    testResult = null
    clearTimeout(alertTimeout)
    const newObject = { ...(dlg.object ?? {}), ...$state.snapshot(dlg.form) }
    try {
      const result = await api.request<{ success: boolean }>('POST', '/api/mail_accounts/test/', newObject)
      testActive = false
      testResult = result.success ? 'success' : 'danger'
    } catch (e: any) {
      testActive = false
      testResult = 'danger'
      dlg.error = e?.body ?? null
    }
    alertTimeout = setTimeout(() => (testResult = null), 5000)
  }
</script>

<pngx-mail-account-edit-dialog><form onsubmit={(e) => { e.preventDefault(); dlg.save() }} autocomplete="off">
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
      <div class="col">
        <pngx-input-text><InputText title={t('common.name')} bind:value={dlg.form.name} error={dlg.error?.name} autocomplete="off" /></pngx-input-text>
        <pngx-input-text><InputText title={t('dialog.mail_account.imap_server')} bind:value={dlg.form.imap_server} error={dlg.error?.imap_server} /></pngx-input-text>
        <pngx-input-text><InputText title={t('dialog.mail_account.imap_port')} bind:value={dlg.form.imap_port} error={dlg.error?.imap_port} /></pngx-input-text>
        <pngx-input-select><InputSelect title={t('dialog.mail_account.imap_security')} items={imapSecurityOptions} bind:value={dlg.form.imap_security} /></pngx-input-select>
      </div>
      <div class="col">
        <pngx-input-text><InputText title={t('common.username')} bind:value={dlg.form.username} error={dlg.error?.username} /></pngx-input-text>
        <pngx-input-password><InputPassword title={t('common.password')} bind:value={dlg.form.password} error={dlg.error?.password} /></pngx-input-password>
        <pngx-input-check><InputCheck title={t('dialog.mail_account.password_token')} hint={t('dialog.mail_account.check_password_above_token')} bind:value={dlg.form.is_token} error={dlg.error?.is_token} /></pngx-input-check>
        <pngx-input-text><InputText title={t('dialog.mail_account.character_set')} bind:value={dlg.form.character_set} error={dlg.error?.character_set} /></pngx-input-text>
      </div>
    </div>
  </div>
  <div class="modal-footer">
    <div class="m-0 me-2">
      {#if testResult}
        <ngb-alert role="alert" class="alert show fade alert-{testResult} alert-dismissible mb-0 py-2">{testResultMessage}<button type="button" class="btn-close" aria-label="Close" onclick={() => (testResult = null)}></button></ngb-alert>
      {/if}
    </div>
    <button type="button" class="btn btn-outline-primary" onclick={test} disabled={dlg.networkActive || testActive}>
      {#if testActive}
        <div class="spinner-border spinner-border-sm me-2" role="status"></div>
        <span class="visually-hidden mr-1">{t('common.loading')}</span>
      {/if}
      {t('dialog.mail_account.test')}
    </button>
    <button type="button" class="btn btn-outline-secondary" onclick={() => dlg.cancel()} disabled={dlg.networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={dlg.networkActive}>{t('common.save')}</button>
  </div>
</form></pngx-mail-account-edit-dialog>
