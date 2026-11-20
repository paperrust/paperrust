<!-- Port of manage/mail/processed-mail-dialog/processed-mail-dialog.component.html (size xl). -->
<script lang="ts">
  import '../../../../styles/components/processed-mail-dialog.css'
  import { t, tRich, locale } from '../../../../lib/i18n'
  import { icon } from '../../../common/bootstrap-icon/icon'
  import { ricon } from '../../../../lib/route-icons'
  import { Popover } from '../../../../lib/popover.svelte'
  import { formatDate } from '../../../../lib/dates'
  import type { ModalRef } from '../../../../lib/modal.svelte'
  import { session, SETTINGS_KEYS } from '../../../../stores/settings.svelte'
  import { toastService } from '../../../../stores/toasts.svelte'
  import { api } from '../../../../api/client'
  import { listObjects } from '../../../../api/admin'
  import ConfirmButton from '../../../common/confirm-button/ConfirmButton.svelte'
  import Pagination from '../../../common/pagination/Pagination.svelte'
  import '../mail-icons'

  let { modal, rule }: { modal: ModalRef<any>; rule: { id: number; name: string } } = $props()
  let processedMails = $state<any[]>([])
  let loading = $state(true)
  let toggleAllEnabled = $state(false)
  let selectedMailIds = $state(new Set<number>())
  let page = $state(1)
  const info = new Popover({ autoClose: true })
  const errPops = new Map<number, Popover>()
  const errPop = (id: number) => {
    let p = errPops.get(id)
    if (!p) errPops.set(id, (p = new Popover({ triggers: 'mouseenter:mouseleave' })))
    return p
  }
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  const heading = $derived(tRich('manage.mail.processed_mail', { INTERPOLATION: rule.name }))

  async function loadProcessedMails() {
    loading = true
    clearSelection()
    const r = await listObjects<any>('processed_mail', page, 50, 'processed_at', true, { rule: rule.id })
    processedMails = r.results
    loading = false
  }
  loadProcessedMails()

  async function deleteSelected() {
    await api.request('POST', '/api/processed_mail/bulk_delete/', { mail_ids: Array.from(selectedMailIds) })
    toastService.showSuccess(t('manage.mail.processed_mail_s_deleted'))
    loadProcessedMails()
  }
  function toggleAll(e: Event) {
    if ((e.target as HTMLInputElement).checked) selectedMailIds = new Set(processedMails.map((m) => m.id))
    else clearSelection()
  }
  function clearSelection() {
    toggleAllEnabled = false
    selectedMailIds = new Set()
  }
  function toggleSelected(mail: any) {
    const s = new Set(selectedMailIds)
    if (s.has(mail.id)) s.delete(mail.id)
    else s.add(mail.id)
    selectedMailIds = s
  }
</script>

<pngx-processed-mail-dialog>
<div class="modal-header">
  <h6 class="modal-title" id="modal-basic-title">{#each heading as seg}{#if seg.tag}<em>{seg.text}</em>{:else}{seg.text}{/if}{/each}</h6>
  <button class="btn btn-sm btn-link text-muted me-auto p-0 p-md-2" title={t("common.whats")} type="button" use:info.triggerEl>
    <pngx-bs-icon name="question-circle">{@html icon('question-circle')}</pngx-bs-icon>
  </button>
  {#if info.open}
    <ngb-popover-window role="tooltip" class="popover" id={info.id} use:info.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
      <a href="rs/docs/mail.md" target="_blank" referrerpolicy={'noopener noreferrer' as any}>{t('common.read_more')}</a>
      <pngx-bs-icon class="ms-1" width=".8em" height=".8em" name="box-arrow-up-right">{@html icon('box-arrow-up-right', '.8em', '.8em')}</pngx-bs-icon>
    </div></ngb-popover-window>
  {/if}
  <button type="button" class="btn-close" aria-label="Close" onclick={() => modal.close()}></button>
</div>
<div class="modal-body">
  {#if loading}
    <div class="text-center my-5">
      <div class="spinner-border" role="status">
        <span class="visually-hidden">{t('common.loading')}</span>
      </div>
    </div>
  {:else if processedMails.length === 0}
    <span>{t('manage.mail.no_processed_email_messages')}</span>
  {:else}
    <div class="table-responsive">
      <table class="table table-hover table-sm align-middle">
        <thead>
          <tr>
            <th scope="col" style={"width: 40px;"}>
              <div class="form-check m-0 ms-2 me-n2">
                <input type="checkbox" class="form-check-input" id="all-objects" bind:checked={toggleAllEnabled} disabled={processedMails.length === 0} onclick={(e) => { toggleAll(e); e.stopPropagation() }}>
                <label class="form-check-label" for="all-objects"><span class="visually-hidden">{t('manage.mail.select_all_processed_email')}</span></label>
              </div>
            </th>
            <th scope="col">{t('common.subject')}</th>
            <th scope="col">{t('manage.mail.received')}</th>
            <th scope="col">{t('manage.mail.processed')}</th>
            <th scope="col">{t('common.status')}</th>
            <th scope="col">{t('common.error')}</th>
          </tr>
        </thead>
        <tbody>
          {#each processedMails as mail (mail.id)}
            <tr>
              <td>
                <div class="form-check m-0 ms-2 me-n2">
                  <input type="checkbox" class="form-check-input" id={mail.id} checked={selectedMailIds.has(mail.id)} onclick={(e) => { toggleSelected(mail); e.stopPropagation() }}>
                  <label class="form-check-label" for={mail.id}></label>
                </div>
              </td>
              <td>{mail.subject}</td>
              <td>{formatDate(mail.received, 'longDate', dateLocale)}</td>
              <td>{formatDate(mail.processed, 'longDate', dateLocale)}</td>
              <td>
                {#if mail.status === 'SUCCESS'}
                  <pngx-bs-icon name="check-circle" title="SUCCESS" class="text-success">{@html ricon('check-circle')}</pngx-bs-icon>
                {:else if mail.status === 'FAILED'}
                  <pngx-bs-icon name="exclamation-triangle" title="FAILED" class="text-danger">{@html ricon('exclamation-triangle')}</pngx-bs-icon>
                {:else}
                  <pngx-bs-icon name="slash-circle" title={mail.status} class="text-muted">{@html ricon('slash-circle')}</pngx-bs-icon>
                {/if}
              </td>
              <td>
                {#if mail.error}
                  {@const p = errPop(mail.id)}
                  <span class="text-danger" use:p.triggerEl>{String(mail.error).slice(0, 20)}</span>
                  {#if p.open}
                    <ngb-popover-window role="tooltip" class="popover" id={p.id} use:p.windowEl><div class="popover-arrow" data-popper-arrow></div><div class="popover-body">
                      <pre class="small">
                    {mail.error}
                  </pre>
                    </div></ngb-popover-window>
                  {/if}
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div class="btn-toolbar">
      <button type="button" class="btn btn-outline-secondary me-2" onclick={clearSelection} disabled={selectedMailIds.size === 0}>{t('common.clear')}</button>
      <pngx-confirm-button title={t('common.delete_selected')}><ConfirmButton
        label={t('common.delete_selected')}
        buttonClasses="btn-outline-danger"
        iconName="trash"
        disabled={selectedMailIds.size === 0}
        onconfirm={deleteSelected} /></pngx-confirm-button>
      {#if processedMails.length > 50}
        <div class="ms-auto">
          <ngb-pagination role="navigation"><Pagination collectionSize={processedMails.length} {page} pageSize={50} maxSize={5} onpage={(p) => { page = p; loadProcessedMails() }} /></ngb-pagination>
        </div>
      {/if}
    </div>
  {/if}
</div>
</pngx-processed-mail-dialog>
