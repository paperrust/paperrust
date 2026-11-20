// Content of <pngx-bs-icon>: inline SVG like BootstrapIconComponent (width/height override).
// The host element <pngx-bs-icon> stands – as in Angular – in the markup of the calling component:
//   <pngx-bs-icon name="house" class="me-2">{@html icon('house')}</pngx-bs-icon>
// Icons outside the core (tools/icons.mjs) are loaded lazily; until then the icon stays empty and the
// markup redraws after loading (extraIconState.version).
import { icons } from './icons.generated'
import { extraIconState, loadExtraIcons } from './icon-extra.svelte'

export function icon(name: string, width?: string | number, height?: string | number): string {
  let svg = icons[name]
  if (!svg) {
    // reactive dependency: re-evaluate after lazy loading
    void extraIconState.version
    if (!extraIconState.loaded) {
      void loadExtraIcons()
      return ''
    }
    console.warn(`Icon not found: ${name}`)
    return ''
  }
  if (width) svg = svg.replace(/ width="[^"]*"/, ` width="${width}"`)
  if (height) svg = svg.replace(/ height="[^"]*"/, ` height="${height}"`)
  return svg
}
