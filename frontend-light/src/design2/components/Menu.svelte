<!-- Dropdown menu: closes on click outside and Esc, arrow keys move through the entries. -->
<script lang="ts">
  import { onMount, type Snippet } from 'svelte'
  import { menuFocusBack } from '../lib/focus'
  let { onclose, label, align = 'right', width = undefined, children }: { onclose: () => void; label: string; align?: 'left' | 'right'; width?: string; children: Snippet } = $props()
  let el: HTMLDivElement
  let pos = $state<{ top: string; left?: string; right?: string; maxh: string }>({ top: '-9999px', maxh: '60vh' })

  /** fixed positioned at the trigger (escapes scrolling containers), kept within the window */
  function place() {
    const anchor = el.parentElement?.getBoundingClientRect()
    if (!anchor) return
    const w = el.offsetWidth
    const below = innerHeight - anchor.bottom - 12
    const above = anchor.top - 12
    const up = below < 220 && above > below
    const maxh = Math.max(160, Math.min(520, up ? above : below))
    const top = up ? Math.max(8, anchor.top - 6 - Math.min(el.scrollHeight, maxh)) : anchor.bottom + 6
    let left = align === 'left' ? anchor.left : anchor.right - w
    left = Math.max(8, Math.min(left, innerWidth - w - 8))
    pos = { top: `${top}px`, left: `${left}px`, maxh: `${maxh}px` }
  }

  const items = () => [...el.querySelectorAll<HTMLElement>('.d2-menu-item, button, a[href], input')].filter((x) => !x.hasAttribute('disabled'))
  onMount(() => {
    const prev = document.activeElement as HTMLElement | null
    place()
    items()[0]?.focus({ preventScroll: true })
    const re = () => place()
    addEventListener('resize', re)
    addEventListener('scroll', re, true)
    const down = (e: PointerEvent) => {
      const opener = prev && prev.contains(e.target as Node)
      if (!el.contains(e.target as Node) && !opener) onclose()
    }
    document.addEventListener('pointerdown', down, true)
    return () => {
      document.removeEventListener('pointerdown', down, true)
      removeEventListener('resize', re)
      removeEventListener('scroll', re, true)
      // DOM is already removed here (Svelte 5) → focus may be on <body>; then back to the trigger
      menuFocusBack(document.activeElement, document.body, el, prev)?.focus({ preventScroll: true })
    }
  })
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onclose()
      return
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
    if ((e.target as HTMLElement).tagName === 'INPUT' && e.key === 'ArrowUp') return
    const list = items()
    const i = list.indexOf(document.activeElement as HTMLElement)
    const n = e.key === 'ArrowDown' ? (i + 1) % list.length : (i - 1 + list.length) % list.length
    list[n]?.focus()
    e.preventDefault()
  }
</script>

<div class="d2-menu" role="menu" tabindex="-1" aria-label={label} bind:this={el} {onkeydown} style:width style:top={pos.top} style:left={pos.left} style:max-height={pos.maxh}>
  {@render children()}
</div>

<style>
  .d2-menu {
    position: fixed;
    z-index: 70;
    overflow: auto;
  }
</style>
