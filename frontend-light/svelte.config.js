import { vitePreprocess } from '@sveltejs/vite-plugin-svelte'
import { ngMarkup } from './tools/ng-markup.js'

// Markup is deliberately adopted 1:1 from Angular/ng-bootstrap (image parity); these a11y hints concern
// exactly this original markup and are compared with axe against Angular in P4 (FRONTEND-LIGHT §9).
const PARITY_WARNINGS = new Set(['a11y_invalid_attribute', 'a11y_no_redundant_roles', 'a11y_role_has_required_aria_props'])

export default {
  // ngMarkup first: whitespace rules and encapsulation attributes like Angular (pixel-identical)
  preprocess: [ngMarkup(), vitePreprocess()],
  compilerOptions: { runes: true },
  onwarn: (w, handler) => {
    if (PARITY_WARNINGS.has(w.code)) return
    handler(w)
  },
}
