<!-- Port of common/profile-edit-dialog (ProfileEditDialogComponent): GET/PATCH /api/profile/, auth token,
     social accounts, TOTP (/api/profile/totp/). Opened from AppFrame ("My Profile", backdrop static, size xl).
     Host <pngx-profile-edit-dialog> is here because ModalHost renders the component directly. -->
<script lang="ts">
  import { onMount } from 'svelte'
  import { t } from '../../../lib/i18n'
  import { icon } from '../bootstrap-icon/icon'
  import type { ModalRef } from '../../../lib/modal.svelte'
  import { toastService } from '../../../stores/toasts.svelte'
  import { BASE } from '../../../router.svelte'
  import {
    profileGet,
    profileUpdate,
    profileGenerateAuthToken,
    profileDisconnectSocialAccount,
    profileSocialAccountProviders,
    profileTotpSettings,
    profileActivateTotp,
    profileDeactivateTotp,
    type SocialAccount,
    type SocialAccountProvider,
    type TotpSettings,
  } from '../../../api/admin'
  import { ApiError } from '../../../api/v1'
  import InputText from '../input/text/InputText.svelte'
  import InputPassword from '../input/password/InputPassword.svelte'
  import ConfirmButton from '../confirm-button/ConfirmButton.svelte'
  import { confirmMismatch, logoutUrl, profilePayload, qrSvgDataUrl, type ProfileForm } from './profile-edit'

  let { modal }: { modal: ModalRef<any> } = $props()

  let networkActive = $state(false)
  let error = $state<Record<string, any> | null | undefined>(undefined)
  let copied = $state(false)
  let codesCopied = $state(false)
  let hasUsablePassword = $state(false)
  let isTotpEnabled = $state(false)
  let totpSettings = $state<TotpSettings | undefined>(undefined)
  let totpSettingsLoading = $state(false)
  let totpLoading = $state(false)
  let totpOpen = $state(false)
  let recoveryCodes = $state<string[] | null>(null)
  let socialAccounts = $state<SocialAccount[]>([])
  let socialAccountProviders = $state<SocialAccountProvider[]>([])

  let form = $state<ProfileForm>({ email: '', email_confirm: null, password: null, password_confirm: null, first_name: '', last_name: '', auth_token: '', totp_code: '' })
  let currentEmail = $state<string | undefined>(undefined)
  let currentPassword = $state<string | undefined>(undefined)

  // onEmailChange / onPasswordChange: confirmation field only on change, error as long as it does not match
  const showEmailConfirm = $derived(currentEmail !== undefined && currentEmail !== form.email)
  const showPasswordConfirm = $derived(currentPassword !== undefined && (form.password ?? undefined) !== currentPassword)
  const emailConfirmError = $derived(confirmMismatch(showEmailConfirm, form.email, form.email_confirm) ? t('dialog.profile.emails_must_match') : undefined)
  const passwordConfirmError = $derived(confirmMismatch(showPasswordConfirm, form.password, form.password_confirm) ? t('dialog.profile.passwords_must_match') : undefined)
  const saveDisabled = $derived(!!(emailConfirmError || passwordConfirmError))
  const qrUrl = $derived(qrSvgDataUrl(totpSettings?.qr_svg))
  const fieldError = (name: string) => (name === 'email_confirm' ? emailConfirmError : name === 'password_confirm' ? passwordConfirmError : undefined) ?? error?.[name]

  onMount(() => {
    networkActive = true
    profileGet()
      .then((profile) => {
        networkActive = false
        form.email = profile.email ?? ''
        form.password = profile.password ?? null
        form.first_name = profile.first_name ?? ''
        form.last_name = profile.last_name ?? ''
        form.auth_token = profile.auth_token ?? ''
        currentEmail = profile.email ?? ''
        currentPassword = profile.password ?? undefined
        hasUsablePassword = !!profile.has_usable_password
        socialAccounts = profile.social_accounts ?? []
        isTotpEnabled = !!profile.is_mfa_enabled
      })
      .catch(() => (networkActive = false))
    profileSocialAccountProviders()
      .then((providers) => (socialAccountProviders = providers ?? []))
      .catch(() => {})
  })

  async function save(e?: Event) {
    e?.preventDefault()
    if (networkActive || saveDisabled) return
    const passwordChanged = !!form.password && currentPassword !== form.password
    const profile = profilePayload(form, showEmailConfirm, showPasswordConfirm)
    error = null
    networkActive = true
    try {
      await profileUpdate(profile)
      toastService.showSuccess(t('dialog.profile.saved'))
      if (passwordChanged) {
        toastService.showInfo(t('admin.users.password_changed_logged_out'))
        setTimeout(() => (location.href = logoutUrl(BASE)), 2500)
      }
      modal.close()
    } catch (err) {
      toastService.showError(t('dialog.profile.save_failed'), err)
      error = err instanceof ApiError ? (err.body as Record<string, any>) : undefined
      networkActive = false
    }
  }

  async function generateAuthToken() {
    try {
      form.auth_token = await profileGenerateAuthToken()
    } catch (err) {
      toastService.showError(t('dialog.profile.token_failed'), err)
    }
  }

  async function copyText(text: string) {
    try {
      await navigator.clipboard?.writeText(text)
    } catch {}
  }
  function copyAuthToken() {
    copyText(form.auth_token ?? '')
    copied = true
    setTimeout(() => (copied = false), 3000)
  }

  async function disconnectSocialAccount(id: number) {
    try {
      const removed = await profileDisconnectSocialAccount(id)
      socialAccounts = socialAccounts.filter((account) => account.id != removed)
    } catch (err) {
      toastService.showError(t('dialog.profile.disconnect_failed'), err)
    }
  }

  // ngbAccordionButton (click)="gettotpSettings()": every click toggles and reloads
  async function gettotpSettings() {
    totpOpen = !totpOpen
    totpSettingsLoading = true
    try {
      totpSettings = await profileTotpSettings()
    } catch (err) {
      toastService.showError(t('dialog.profile.totp_load_failed'), err)
    }
    totpSettingsLoading = false
  }

  async function activateTotp() {
    if (!totpSettings) return
    totpLoading = true
    try {
      const res = await profileActivateTotp(totpSettings.secret, form.totp_code ?? '')
      isTotpEnabled = res.success
      recoveryCodes = res.recovery_codes
      if (res.success) toastService.showSuccess(t('dialog.profile.totp_activated'))
      else toastService.showError(t('dialog.profile.totp_activate_failed'))
    } catch (err) {
      toastService.showError(t('dialog.profile.totp_activate_failed'), err)
    }
    totpLoading = false
  }

  async function deactivateTotp() {
    totpLoading = true
    try {
      const success = await profileDeactivateTotp()
      isTotpEnabled = !success
      recoveryCodes = null
      if (success) toastService.showSuccess(t('dialog.profile.totp_deactivated'))
      else toastService.showError(t('dialog.profile.totp_deactivate_failed'))
    } catch (err) {
      toastService.showError(t('dialog.profile.totp_deactivate_failed'), err)
    }
    totpLoading = false
  }

  function copyRecoveryCodes() {
    copyText((recoveryCodes ?? []).join('\n'))
    codesCopied = true
    setTimeout(() => (codesCopied = false), 3000)
  }
</script>

<pngx-profile-edit-dialog><form onsubmit={save} autocomplete="off">
  <div class="modal-header">
    <h4 class="modal-title" id="modal-basic-title">{t('dialog.profile.title')}</h4>
    <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}>
    </button>
  </div>
  <div class="modal-body">
    <div class="row">
      <div class="col-12 col-md-6">
        <pngx-input-text><InputText title={t('common.email')} bind:value={form.email} error={fieldError('email')} /></pngx-input-text>
        {#if showEmailConfirm}
          <div class="accordion">
            <div class="accordion-item border-0 bg-transparent">
              <div class="accordion-collapse collapse show">
                <div class="accordion-body p-0">
                  <pngx-input-text><InputText title={t('dialog.profile.confirm_email')} bind:value={form.email_confirm} autocomplete="email" error={fieldError('email_confirm')} /></pngx-input-text>
                </div>
              </div>
            </div>
          </div>
        {/if}
        <pngx-input-password><InputPassword title={t('common.password')} bind:value={form.password} showReveal={true} autocomplete="current-password" error={fieldError('password')} /></pngx-input-password>
        {#if showPasswordConfirm}
          <div class="accordion">
            <div class="accordion-item border-0 bg-transparent">
              <div class="accordion-collapse collapse show">
                <div class="accordion-body p-0">
                  <pngx-input-password><InputPassword title={t('dialog.profile.confirm_password')} bind:value={form.password_confirm} autocomplete="new-password" error={fieldError('password_confirm')} /></pngx-input-password>
                </div>
              </div>
            </div>
          </div>
        {/if}
        <pngx-input-text><InputText title={t('dialog.user.first_name')} bind:value={form.first_name} error={fieldError('first_name')} /></pngx-input-text>
        <!-- upstream shows error()?.first_name here (typo); we show the error of the field itself -->
        <pngx-input-text><InputText title={t('dialog.user.last_name')} bind:value={form.last_name} error={fieldError('last_name')} /></pngx-input-text>
        <div class="mb-3">
          <label class="form-label" for="profile-auth-token">{t('dialog.profile.api_token')}</label>
          <div class="position-relative">
            <div class="input-group">
              <input type="text" class="form-control" id="profile-auth-token" value={form.auth_token} readonly>
              <button type="button" class="btn btn-outline-secondary" onclick={copyAuthToken} title={t('common.copy')}>
                {#if !copied}
                  <pngx-bs-icon width="1em" height="1em" name="clipboard-fill">{@html icon('clipboard-fill', '1em', '1em')}</pngx-bs-icon>
                {/if}
                {#if copied}
                  <pngx-bs-icon width="1em" height="1em" name="clipboard-check-fill">{@html icon('clipboard-check-fill', '1em', '1em')}</pngx-bs-icon>
                {/if}
                <span class="visually-hidden">{t('common.copy')}</span>
              </button>
              <pngx-confirm-button title={t('dialog.profile.regenerate_token')}><ConfirmButton buttonClasses=" btn-outline-secondary" iconName="arrow-repeat" onconfirm={generateAuthToken} /></pngx-confirm-button>
            </div>
            <span class="badge copied-badge bg-primary small fade ms-4 position-absolute top-50 translate-middle-y pe-none z-3" style={"right: 8em;"} class:show={copied}>{t('ui.copied')}</span>
          </div>
          <div class="form-text text-muted text-end fst-italic">{t('dialog.profile.token_warning')}</div>
        </div>
      </div>
      <div class="col-12 col-md-6">
        {#if socialAccounts.length > 0}
          <div class="mb-3">
            <p>{t('dialog.profile.linked_accounts')}</p>
            <ul class="list-group">
              {#each socialAccounts as account (account.id)}
                <li class="list-group-item" title={hasUsablePassword ? undefined : t('dialog.profile.set_password_first')}>
                  {account.name} ({account.provider})
                  <pngx-confirm-button title={t('dialog.profile.disconnect_account', { INTERPOLATION: account.name })}><ConfirmButton label={t('dialog.profile.disconnect')} buttonClasses="btn-outline-danger btn-sm ms-2 align-baseline" iconName="trash" disabled={!hasUsablePassword} onconfirm={() => disconnectSocialAccount(account.id)} /></pngx-confirm-button>
                </li>
              {/each}
            </ul>
            <div class="form-text text-muted text-end fst-italic">{t('dialog.profile.disconnect_warning')}</div>
          </div>
        {/if}
        {#if socialAccountProviders.length > 0}
          <div class="mb-3">
            <p>{t('dialog.profile.link_account')}</p>
            <div class="list-group">
              {#each socialAccountProviders as provider (provider.name)}
                <a class="list-group-item list-group-item-action text-primary d-flex align-items-center" href={provider.login_url} rel="noopener noreferrer">
                  {provider.name}<pngx-bs-icon class="pb-1 ms-2" name="box-arrow-up-right">{@html icon('box-arrow-up-right')}</pngx-bs-icon>
                </a>
              {/each}
            </div>
          </div>
        {/if}
        {#if !isTotpEnabled}
          <div class="accordion">
            <div class="accordion-item">
              <h2 class="accordion-header">
                <button type="button" class="accordion-button" class:collapsed={!totpOpen} aria-expanded={totpOpen} onclick={gettotpSettings}>{t('dialog.user.two_factor_authentication')}</button>
              </h2>
              {#if totpOpen}
                <div class="accordion-collapse collapse show">
                  <div class="accordion-body">
                    {#if totpSettingsLoading}
                      <div class="spinner-border spinner-border-sm fw-normal ms-2 me-auto" role="status"></div>
                      <div class="visually-hidden">{t('common.loading')}</div>
                    {:else if totpSettings}
                      <figure class="figure">
                        {#if qrUrl}
                          <img class="bg-white d-inline-block" src={qrUrl} alt="Authenticator QR code">
                        {/if}
                        <figcaption class="figure-caption text-end mt-2">{t('dialog.profile.totp_scan_hint')}</figcaption>
                      </figure>
                      <p>
                        {t('dialog.profile.totp_secret')}: <code>{totpSettings.secret}</code>.
                        {t('dialog.profile.totp_secret_hint')}
                      </p>
                      <div class="input-group">
                        <input type="text" class="form-control" bind:value={form.totp_code} placeholder={t('tools.code')} disabled={totpLoading} aria-label={t('tools.code')}>
                        <button type="button" class="btn btn-primary ml-auto" onclick={activateTotp} disabled={totpLoading}>
                          {t('admin.config.enable')}
                          {#if totpLoading}
                            <div class="spinner-border spinner-border-sm fw-normal ms-2" role="status"></div>
                            <div class="visually-hidden">{t('common.loading')}</div>
                          {/if}
                        </button>
                      </div>
                    {/if}
                  </div>
                </div>
              {/if}
            </div>
          </div>
        {:else}
          <span class="d-block mb-2 form-label">{t('dialog.user.two_factor_authentication')}</span>
          {#if recoveryCodes}
            <div class="alert alert-warning" role="alert">
              <pngx-bs-icon name="exclamation-triangle" class="me-1">{@html icon('exclamation-triangle')}</pngx-bs-icon>{t('dialog.profile.recovery_codes_hint')}
            </div>
            <div class="d-flex flex-row align-items-start mb-3">
              <ul class="list-group w-50">
                {#each recoveryCodes as code, i (code)}
                  {#if i % 2 === 0}
                    <li class="list-group-item d-flex justify-content-around align-items-center">
                      <code>{code}</code>
                      {#if recoveryCodes[i + 1]}
                        <code>{recoveryCodes[i + 1]}</code>
                      {/if}
                    </li>
                  {/if}
                {/each}
              </ul>
              <button type="button" class="btn btn-sm btn-outline-secondary ms-2" onclick={copyRecoveryCodes} title={t('common.copy')}>
                {#if !codesCopied}
                  <pngx-bs-icon width="1em" height="1em" name="clipboard-fill" class="me-1">{@html icon('clipboard-fill', '1em', '1em')}</pngx-bs-icon><span>{t('dialog.profile.copy_codes')}</span>
                {/if}
                {#if codesCopied}
                  <pngx-bs-icon width="1em" height="1em" name="clipboard-check-fill" class="text-primary me-1">{@html icon('clipboard-check-fill', '1em', '1em')}</pngx-bs-icon><span class="text-primary">{t('ui.copied')}</span>
                {/if}
              </button>
            </div>
          {/if}
          <pngx-confirm-button title={t('dialog.user.disable_two_factor_authentication')}><ConfirmButton label={t('dialog.user.disable_two_factor_authentication')} buttonClasses="btn-outline-danger btn-sm" iconName="trash" disabled={totpLoading} onconfirm={deactivateTotp} /></pngx-confirm-button>
        {/if}
      </div>
    </div>
  </div>
  <div class="modal-footer">
    <button type="button" class="btn btn-outline-secondary" onclick={() => modal.close()} disabled={networkActive}>{t('common.cancel')}</button>
    <button type="submit" class="btn btn-primary" disabled={networkActive || saveDisabled}>{t('common.save')}</button>
  </div>
</form></pngx-profile-edit-dialog>
