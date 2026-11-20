<!-- Port of common/custom-field-display (without tooltip; the component reloads document links itself). -->
<script lang="ts">
  import '../../../styles/components/custom-field-display.css'
  import { t, locale } from '../../../lib/i18n'
  import { formatDate } from '../../../lib/dates'
  import { icon } from '../bootstrap-icon/icon'
  import type { Document } from '../../../api/v1'
  import { data } from '../../../stores/data.svelte'
  import { session, SETTINGS_KEYS } from '../../../stores/settings.svelte'
  import { api } from '../../../api/client'
  import { router } from '../../../router.svelte'
  import { safeHref } from '../../../lib/safe-href'

  let { document: d, fieldId, showNameIfEmpty = false }: { document: Document; fieldId: number; showNameIfEmpty?: boolean } = $props()

  // Currency of the locale like Angular getLocaleCurrencyCode
  const LOCALE_CURRENCY: Record<string, string> = { 'en-US': 'USD', 'en-GB': 'GBP', 'de-CH': 'CHF', 'fr-CH': 'CHF', 'it-CH': 'CHF', 'ja-JP': 'JPY', 'pl-PL': 'PLN', 'cs-CZ': 'CZK', 'sv-SE': 'SEK', 'da-DK': 'DKK', 'no-NO': 'NOK', 'hu-HU': 'HUF', 'ro-RO': 'RON', 'bg-BG': 'BGN', 'tr-TR': 'TRY', 'ru-RU': 'RUB', 'uk-UA': 'UAH', 'pt-BR': 'BRL', 'zh-CN': 'CNY', 'zh-TW': 'TWD', 'ko-KR': 'KRW', 'he-IL': 'ILS', 'th-TH': 'THB', 'vi-VN': 'VND', 'id-ID': 'IDR', 'ms-MY': 'MYR', 'hi-IN': 'INR', 'fa-IR': 'IRR', 'sr-CS': 'RSD', 'mk-MK': 'MKD', 'af-ZA': 'ZAR', 'xh-ZA': 'ZAR' }

  const field = $derived(data.customFields.find((f) => f.id === fieldId))
  const raw = $derived(d.custom_fields?.find((f) => f.field === fieldId)?.value as any)
  const currency = $derived.by(() => {
    if (!raw || field?.data_type !== 'monetary') return ''
    return String(raw).match(/([A-Z]{3})/)?.[0] ?? field.extra_data?.default_currency ?? LOCALE_CURRENCY[locale()] ?? 'EUR'
  })
  const value = $derived(field?.data_type === 'monetary' && raw ? parseFloat(String(raw).replace(currency, '')) : raw)
  const dateLocale = $derived(session.get(SETTINGS_KEYS.DATE_LOCALE) || locale())
  let docTitles = $state<Record<number, string>>({})
  $effect(() => {
    if (field?.data_type === 'documentlink' && Array.isArray(raw) && raw.length) {
      api.get<{ results: { id: number; title: string }[] }>(`/api/documents/?id__in=${raw.join(',')}&fields=id,title`).then((r) => {
        docTitles = Object.fromEntries(r.results.map((x) => [x.id, x.title]))
      }).catch(() => {})
    }
  })
  /** Angular CurrencyPipe (symbol, 2 decimal places) */
  function money(v: number, cur: string) {
    try {
      return new Intl.NumberFormat(locale(), { style: 'currency', currency: cur, currencyDisplay: 'symbol' }).format(v)
    } catch {
      return `${cur}${v}`
    }
  }
</script>

{#if field}
    {#if value?.toString().length > 0}
        {#if field.data_type === 'monetary'}
                <span>{money(value, currency)}</span>
        {:else if field.data_type === 'date'}
                <span>{formatDate(value, session.get(SETTINGS_KEYS.DATE_FORMAT), dateLocale)}</span>
        {:else if field.data_type === 'url'}
                {#if safeHref(value)}
                <a href={safeHref(value)} class="btn-link text-dark text-decoration-none" target="_blank" rel="noopener noreferrer">{value}</a>
                {:else}
                <span>{value}</span>
                {/if}
        {:else if field.data_type === 'documentlink'}
                <div class="d-flex gap-1 flex-wrap">
                    {#each value as docId}
                      {#if docTitles[docId]}
                          <a href={router.href(`documents/${docId}`)} class="badge bg-body text-primary" title={t('common.view')}>
                              <pngx-bs-icon width="0.9em" height="0.9em" name="file-text" class="me-1">{@html icon('file-text', '0.9em', '0.9em')}</pngx-bs-icon><span>{docTitles[docId]}</span>
                          </a>
                      {/if}
                    {/each}
                </div>
        {:else if field.data_type === 'boolean'}
                <div class="d-flex flex-row align-items-center">
                    <span>{field.name}:</span>
                    <input type="checkbox" id={field.name} name={field.name} checked={!!value} value="" class="form-check-input ms-2 mt-0 pe-none">
                </div>
        {:else if field.data_type === 'select'}
                <span>{field.extra_data?.select_options?.find((o: { id: string; label: string }) => o.id === value)?.label}</span>
        {:else if field.data_type === 'longtext'}
                <p class="mb-0">{String(value).slice(0, 20)}{String(value).length > 20 ? '...' : ''}</p>
        {:else}
              <span>{value}</span>
        {/if}
    {:else if showNameIfEmpty}
        <span class="fst-italic text-muted">{field.name}</span>
    {/if}
{/if}
