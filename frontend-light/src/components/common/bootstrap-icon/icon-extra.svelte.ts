// Lazy loading of the rarely used icons (icons-extra.generated.ts, see tools/icons.mjs): the core belongs
// to the first call (budget), the rest comes as its own chunk – when idle after start (main.ts) or on the
// first icon('…') that is missing in the core. `version` is reactive: markup with `{@html icon('…')}` reads it when the
// icon is still missing, and redraws after loading.
import { icons } from './icons.generated'

export const extraIconState = $state({ version: 0, loaded: false })

let loading: Promise<void> | undefined

export function loadExtraIcons(): Promise<void> {
  loading ??= import('./icons-extra.generated').then(
    (m) => {
      for (const [k, v] of Object.entries(m.extraIcons)) if (!icons[k]) icons[k] = v
      extraIconState.loaded = true
      extraIconState.version++
    },
    (e) => {
      // next attempt at the next missing icon (e.g. after a short network outage)
      loading = undefined
      console.warn('Icons could not be loaded', e)
    },
  )
  return loading
}
