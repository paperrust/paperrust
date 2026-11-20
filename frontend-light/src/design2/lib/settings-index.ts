// Directory of the settings for the global search (feedback 2026-11-18: "search settings, click opens the
// menu and the setting flashes"). Static entries for the pages of the UI, plus – for administrators
// only – all server settings from /rs/api/v2/settings (page settings/system, row #sys-<key>).
import { msg, t } from '../../lib/i18n'
import { session } from '../../stores/settings.svelte'
import { fuzzyScore, rank } from './palette'

export interface SettingEntry {
  id: string
  /** displayed name (already translated) */
  label: string
  /** Area, e.g. "Einstellungen › Dokumente" */
  where: string
  keywords?: string
  route: string
  /** CSS selector of the target; otherwise the text of `anchor` is searched */
  selector?: string
  anchor?: string
}

type Raw = { k: string; route: string; where: string; kw?: string }

// "Appearance" page (design2/settings/SettingsHost.svelte)
const DISPLAY: Raw[] = [
  { k: 'm.settings.interface_design', route: 'settings', where: 'display', kw: 'design klassisch modern oberfläche interface' },
  { k: 'm.settings.color_scheme', route: 'settings', where: 'display', kw: 'dunkel hell theme dark light farbe' },
  { k: 'm.settings.density', route: 'settings', where: 'display', kw: 'kompakt abstand dichte compact' },
  { k: 'common.language', route: 'settings', where: 'display', kw: 'sprache language' },
  { k: 'm.settings.home_customize', route: 'settings', where: 'display', kw: 'startseite bausteine widgets dashboard' },
  { k: 'common.keyboard_shortcuts', route: 'settings', where: 'display', kw: 'tasten kürzel shortcuts' },
  { k: 'common.account', route: 'settings/account', where: 'account', kw: 'profil passwort 2fa zwei-faktor token konto' },
]

// Classic settings component (components/admin/settings/Settings.svelte), sections general/documents/permissions/notifications
const CLASSIC: Raw[] = [
  { k: 'admin.settings.display_language', route: 'settings/general', where: 'general', kw: 'sprache' },
  { k: 'admin.settings.date_display', route: 'settings/general', where: 'general', kw: 'datum anzeige' },
  { k: 'admin.settings.date_format', route: 'settings/general', where: 'general', kw: 'datumsformat' },
  { k: 'admin.settings.sidebar', route: 'settings/general', where: 'general', kw: 'seitenleiste navigation' },
  { k: 'admin.settings.dark_mode', route: 'settings/general', where: 'general', kw: 'dunkel' },
  { k: 'admin.settings.theme_color', route: 'settings/general', where: 'general', kw: 'farbe akzent' },
  { k: 'admin.settings.global_search', route: 'settings/general', where: 'general', kw: 'suche' },
  { k: 'admin.settings.update_checking', route: 'settings/general', where: 'general', kw: 'updates aktualisierung' },
  { k: 'common.saved_views', route: 'settings/general', where: 'general', kw: 'ansichten' },
  { k: 'admin.settings.items_per_page', route: 'settings/documents', where: 'documents', kw: 'seitengrösse anzahl' },
  { k: 'admin.settings.document_editing', route: 'settings/documents', where: 'documents', kw: 'pdf viewer zoom' },
  { k: 'admin.settings.default_zoom', route: 'settings/documents', where: 'documents', kw: 'zoom' },
  { k: 'admin.settings.automatically_remove_inbox_tag', route: 'settings/documents', where: 'documents', kw: 'posteingang inbox' },
  { k: 'admin.settings.suggestions_default', route: 'settings/documents', where: 'documents', kw: 'vorschläge ki' },
  { k: 'admin.settings.bulk_editing', route: 'settings/documents', where: 'documents', kw: 'massenbearbeitung bestätigung' },
  { k: 'admin.settings.default_editing_mode', route: 'settings/documents', where: 'documents', kw: 'version neues dokument' },
  { k: 'admin.settings.enable_notes', route: 'settings/documents', where: 'documents', kw: 'notizen' },
  { k: 'admin.settings.default_permissions', route: 'settings/permissions', where: 'permissions', kw: 'rechte besitzer' },
  { k: 'admin.settings.default_owner', route: 'settings/permissions', where: 'permissions', kw: 'besitzer eigentümer' },
  { k: 'admin.settings.default_view_permissions', route: 'settings/permissions', where: 'permissions', kw: 'ansehen rechte' },
  { k: 'admin.settings.default_edit_permissions', route: 'settings/permissions', where: 'permissions', kw: 'bearbeiten rechte' },
  { k: 'admin.settings.document_processing', route: 'settings/notifications', where: 'notifications', kw: 'benachrichtigungen verarbeitung' },
]

const WHERE: Record<string, string> = {
  display: msg('m.settings.display'),
  account: msg('common.account'),
  general: msg('common.general'),
  documents: msg('m.settings.doc_display_editing'),
  permissions: msg('admin.settings.default_permissions'),
  notifications: msg('common.notifications'),
  ai: msg('m.ai_settings.tab'),
  system: msg('common.system'),
}

/** Entries of the UI (translated) */
export function staticEntries(): SettingEntry[] {
  const settings = t('common.settings')
  return [...DISPLAY, ...CLASSIC].map((r) => ({
    id: 'set-' + r.k,
    label: t(r.k),
    anchor: t(r.k),
    where: `${settings} › ${t(WHERE[r.where] ?? r.where)}`,
    keywords: r.kw,
    route: r.route,
  }))
}

let cache: Promise<SettingEntry[]> | null = null

/** Server settings (only for those who may see them); loaded once per session */
export function systemEntries(): Promise<SettingEntry[]> {
  const admin = !!session.data?.user?.is_superuser || session.can('view', 'applicationconfiguration')
  if (!admin) return Promise.resolve([])
  cache ??= Promise.all([import('../../components/admin/settings/system/api'), import('../../components/admin/settings/system/system')])
    .then(async ([api, sys]) => {
      const d = await api.listSettings()
      const where = `${t('common.settings')} › ${t(WHERE.system)}`
      return d.items
        .filter((i) => !i.alias_of)
        .map((i) => ({
          id: 'sys-' + i.key,
          label: sys.helpText(i) || sys.displayName(i),
          where: `${where} › ${sys.displayName(i)}`,
          keywords: [i.key, sys.displayName(i), ...i.aliases, i.category].join(' '),
          route: `settings/system/${sys.groupOf(i)}`,
          selector: `#sys-${CSS.escape(i.key)}`,
        }))
    })
    .catch(() => {
      cache = null
      return []
    })
  return cache
}

const norm = (s: string) => s.toLowerCase().replace(/\s+/g, ' ').replace(/[:：…]+$/, '').trim()

/** Find the target on the opened page: selector or label with this text */
export function findTarget(e: Pick<SettingEntry, 'selector' | 'anchor'>, root: ParentNode = document): HTMLElement | null {
  if (e.selector) {
    const el = root.querySelector<HTMLElement>(e.selector)
    if (el) return (el.closest('.border-bottom, .row, .form-check, .mb-3, .f, fieldset, section') as HTMLElement | null) ?? el
  }
  if (!e.anchor) return null
  const want = norm(e.anchor)
  const cands = root.querySelectorAll<HTMLElement>('main label, main legend, main h2, main h3, main h4, main h5, main h6, main dt, main th, main .d2-label, main .form-label, main .form-check-label, main summary, main p, main span')
  let best: HTMLElement | null = null
  for (const c of cands) {
    if (!c.offsetParent) continue
    const txt = norm(c.textContent ?? '')
    if (txt === want) {
      best = c
      break
    }
    if (!best && txt.startsWith(want) && txt.length < want.length + 40) best = c
  }
  if (!best) return null
  return (best.closest('.row, .form-check, .mb-3, .f, fieldset, section, .card, .border-bottom') as HTMLElement | null) ?? best
}

/** After the page change: search the target (up to ~4 s), bring it to the center and let it flash briefly */
export async function flashSetting(e: Pick<SettingEntry, 'selector' | 'anchor'>): Promise<boolean> {
  for (let n = 0; n < 40; n++) {
    const el = findTarget(e)
    if (el) {
      el.scrollIntoView({ block: 'center', behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth' })
      el.classList.remove('pr-flash')
      void el.offsetWidth
      el.classList.add('pr-flash')
      setTimeout(() => el.classList.remove('pr-flash'), 2600)
      // Pages often load lazily (list, language file) and shift the target: re-adjust twice if not visible
      for (const ms of [400, 1100]) {
        setTimeout(() => {
          const again = findTarget(e) ?? el
          const r = again.getBoundingClientRect()
          if (r.top < 0 || r.bottom > innerHeight) again.scrollIntoView({ block: 'center' })
          if (again !== el && !again.classList.contains('pr-flash')) {
            again.classList.add('pr-flash')
            setTimeout(() => again.classList.remove('pr-flash'), 2600)
          }
        }, ms)
      }
      const focusable = el.querySelector<HTMLElement>('input, select, textarea, button')
      focusable?.focus({ preventScroll: true })
      return true
    }
    await new Promise((r) => setTimeout(r, 100))
  }
  return false
}

/** Search in the settings: first entries that contain every word (name, area, key, keywords),
 *  hits in the name before hits only in the keywords; without such hits the fuzzy search with a high threshold. */
export function searchSettings(entries: SettingEntry[], query: string, limit = 8): SettingEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter((w) => w.length > 1)
  if (!words.length) return []
  const scored: { e: SettingEntry; s: number }[] = []
  for (const e of entries) {
    const name = e.label.toLowerCase()
    const all = `${name} ${e.where.toLowerCase()} ${(e.keywords ?? '').toLowerCase()}`
    if (!words.every((w) => all.includes(w))) continue
    const inName = words.filter((w) => name.includes(w)).length
    const starts = name.startsWith(words[0]) ? 2 : 0
    scored.push({ e, s: inName * 3 + starts - name.length / 200 })
  }
  if (scored.length) return scored.sort((a, b) => b.s - a.s).slice(0, limit).map((x) => x.e)
  return rank(entries.map((e) => ({ ...e, group: '' })), query, limit).filter((e) => fuzzyScore(query, e.label) >= 60)
}
