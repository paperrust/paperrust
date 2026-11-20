// Icons for lazily loaded screens (administration): the shared icon list (icons.generated.ts)
// belongs to the first call and contains only icons from `icon('…')`. Screens that need further icons
// only in their route register them here via `?raw` import from bootstrap-icons (same SVG files as
// tools/icons.mjs) and write `ricon('…')` in the markup – so they land in the route chunk instead of the first call.
// The check against the Angular registration list is done by route-icons.test.ts.
import { icons } from '../components/common/bootstrap-icon/icons.generated'
import { icon } from '../components/common/bootstrap-icon/icon'

export function registerIcons(set: Record<string, string>) {
  for (const [k, v] of Object.entries(set)) if (!icons[k]) icons[k] = v
}

/** like icon(), for icons that a route brings along via registerIcons */
export const ricon = icon
