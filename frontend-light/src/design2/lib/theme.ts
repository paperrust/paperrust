// Light/dark/system for the PaperRust design. Same settings as the classic UI
// (general-settings:dark-mode:use-system / :enabled), so that both UIs match.
export type ThemeMode = 'light' | 'dark' | 'auto'

export function themeMode(useSystem: boolean, darkEnabled: boolean): ThemeMode {
  return useSystem ? 'auto' : darkEnabled ? 'dark' : 'light'
}

export function nextTheme(m: ThemeMode): ThemeMode {
  return m === 'light' ? 'dark' : m === 'dark' ? 'auto' : 'light'
}

/** data-theme (own tokens) + data-bs-theme (embedded classic components) on <html> */
export function applyTheme(mode: ThemeMode, doc: Document = document) {
  const html = doc.documentElement
  html.setAttribute('data-theme', mode)
  html.setAttribute('data-bs-theme', mode)
}

/** effectively dark? (for thumbnails, meta theme-color) */
export function isDark(mode: ThemeMode, prefersDark: boolean) {
  return mode === 'dark' || (mode === 'auto' && prefersDark)
}

// Density (docs/DESIGN2-ENTWIRREN.md §7): compact/normal, default normal. Stored per user in the
// UI settings under paperrust.density, applied as data-density on <html> (tokens.css adjusts row heights).
export type Density = 'compact' | 'normal'
export const DENSITY_KEY = 'paperrust:density'

/** Raw value from the UI settings → valid density (anything unknown = normal) */
export function densityOf(settings: Record<string, any> | null | undefined): Density {
  return settings?.paperrust?.density === 'compact' ? 'compact' : 'normal'
}

export function applyDensity(d: Density, doc: Document = document) {
  doc.documentElement.setAttribute('data-density', d)
}
