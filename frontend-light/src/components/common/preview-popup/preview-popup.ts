// Logic of common/preview-popup/preview-popup.component.ts (PreviewPopupComponent) without DOM:
// isPdf/renderAsObject and the hover behavior (mouseEnterPreview/mouseLeavePreview/close).

/** The fields of the document that the preview needs */
export interface PreviewDoc {
  id: number
  title?: string | null
  mime_type?: string | null
  archived_file_name?: string | null
}

/** isPdf: archive file present or MIME type contains "pdf" */
export function isPdf(d: PreviewDoc | undefined): boolean {
  return (d?.archived_file_name?.length ?? 0) > 0 || !!d?.mime_type?.includes('pdf')
}

/** Text is loaded via HTTP (init: mime_type contains "text") */
export function isText(d: PreviewDoc | undefined): boolean {
  return !!d?.mime_type?.includes('text')
}

/**
 * Rendering in the popover (template):
 * renderAsObject = (isPdf && useNativePdfViewer) || !isPdf → text (if text MIME) otherwise <object>;
 * otherwise pdf.js viewer.
 */
export function previewKind(d: PreviewDoc | undefined, useNativePdfViewer: boolean): 'text' | 'object' | 'pdfjs' {
  const pdf = isPdf(d)
  const asObject = (pdf && useNativePdfViewer) || !pdf
  if (!asObject) return 'pdfjs'
  return isText(d) ? 'text' : 'object'
}

/** setTimeout in mouseEnterPreview: for this long the popover stays invisible (content is already loading) */
export const SHOW_DELAY_MS = 600
/** close(): delay so the mouse can move from the link into the popover */
export const CLOSE_DELAY_MS = 300

export interface HoverUi {
  isOpen(): boolean
  /** Open popover, but invisible (classes pe-none opacity-0) */
  open(): void
  /** remove invisibility classes */
  reveal(): void
  close(): void
}

/**
 * mouseEnterPreview / mouseLeavePreview / close like upstream. Deviation: leaving the link also
 * closes (delayed, as long as the mouse does not move into the popover); upstream then stays open until a
 * click outside or until the mouse leaves the popover.
 */
export class HoverPreview {
  mouseOnPreview = false
  private timers = new Set<ReturnType<typeof setTimeout>>()

  constructor(private ui: HoverUi) {}

  private later(fn: () => void, ms: number) {
    const id = setTimeout(() => {
      this.timers.delete(id)
      fn()
    }, ms)
    this.timers.add(id)
  }

  enter() {
    this.mouseOnPreview = true
    if (!this.ui.isOpen()) {
      // open but hide, so the content already loads during the wait time
      this.ui.open()
      this.later(() => {
        if (this.mouseOnPreview) this.ui.reveal()
        else this.ui.close()
      }, SHOW_DELAY_MS)
    }
  }

  leave(immediate = false) {
    this.mouseOnPreview = false
    this.later(
      () => {
        if (!this.mouseOnPreview) this.ui.close()
      },
      immediate ? 0 : CLOSE_DELAY_MS
    )
  }

  dispose() {
    this.timers.forEach((id) => clearTimeout(id))
    this.timers.clear()
  }
}
