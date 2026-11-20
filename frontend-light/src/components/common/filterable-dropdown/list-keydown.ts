// FilterableDropdownComponent.listKeyDown / focusNextButtonItem / focusPreviousButtonItem / setButtonItemFocus
// (common/filterable-dropdown/filterable-dropdown.component.ts). Enter/Space on an entry trigger like
// upstream the native click of the <button> (no custom code).
// All entries are rendered (no virtual scrolling), so the index is directly the position.

export interface ListKeyEvent {
  key: string
  shiftKey?: boolean
  target: EventTarget | null
  preventDefault(): void
}

interface Focusable {
  focus(): void
}

export class ListKeyboard {
  /** upstream keyboardIndex */
  index = -1

  /**
   * @param input filter field (listFilterTextInput)
   * @param buttons rendered entry buttons (renderedButtons)
   * @param filterText current filter text
   */
  keyDown(e: ListKeyEvent, input: Focusable | undefined, buttons: Focusable[], filterText: string) {
    const tag = (e.target as { tagName?: string } | null)?.tagName
    const isInput = tag === 'INPUT'
    const isButton = tag === 'BUTTON'
    const sync = () => {
      const i = buttons.indexOf(e.target as unknown as Focusable)
      if (i >= 0) this.index = i
    }
    switch (e.key) {
      case 'ArrowDown':
        if (isInput) {
          const sel = (e.target as { selectionStart?: number | null }).selectionStart
          if (!filterText || sel === filterText.length) {
            this.index = -1
            this.next(buttons)
            e.preventDefault()
          }
        } else if (isButton) {
          sync()
          this.next(buttons)
          e.preventDefault()
        }
        break
      case 'ArrowUp':
        if (isButton) {
          sync()
          if (this.index === 0) input?.focus()
          else this.prev(buttons)
          e.preventDefault()
        }
        break
      case 'Tab':
        // only carry the index along in case arrow keys follow
        if (isInput) this.index = 0
        else if (isButton) {
          if (e.shiftKey) {
            if (this.index > 0) this.prev(buttons, false)
          } else this.next(buttons, false)
        }
        break
    }
  }

  private next(buttons: Focusable[], setFocus = true) {
    this.index = Math.min(buttons.length - 1, this.index + 1)
    if (setFocus) buttons[this.index]?.focus()
  }

  private prev(buttons: Focusable[], setFocus = true) {
    this.index = Math.max(0, this.index - 1)
    if (setFocus) buttons[this.index]?.focus()
  }
}
