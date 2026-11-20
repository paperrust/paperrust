// ngbDropdown container="body": move the menu in <div class="dropdown"> directly under <body> (Popper keeps positioning
// relative to the toggle). Svelte removes the node on closing; the empty wrapper is removed with it.
export function bodyContainer(node: HTMLElement) {
  const wrap = document.createElement('div')
  wrap.className = 'dropdown show'
  document.body.appendChild(wrap)
  wrap.appendChild(node)
  return {
    destroy() {
      wrap.remove()
    },
  }
}
