// Additional attributes from the adopted Angular markup (selectors in styles.scss).
declare module 'svelte/elements' {
  export interface HTMLAttributes<T> {
    pngxsortable?: string
    /** ngx-ui-tour anchor (lib/tour/tour.svelte.ts looks for [tourAnchor="…"]) */
    touranchor?: string
    tourAnchor?: string
  }
}
export {}
