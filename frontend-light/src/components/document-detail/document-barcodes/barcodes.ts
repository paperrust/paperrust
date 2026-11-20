// Logic of document-detail/document-barcodes/document-barcodes.component.ts (framework-free, testable).

/** DocumentBarcodesComponent.isLink: only http(s) URLs with a host are linked */
export function isLink(value: string): boolean {
  try {
    const url = new URL(value.trim())
    return ['http:', 'https:'].includes(url.protocol) && !!url.host
  } catch {
    return false
  }
}

/** Like @angular/cdk Clipboard.copy (return: success?), with navigator.clipboard first. */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // continue with execCommand
  }
  if (typeof document === 'undefined') return false
  const ta = document.createElement('textarea')
  ta.value = text
  ta.setAttribute('readonly', '')
  Object.assign(ta.style, { position: 'fixed', top: '0', left: '-999em', opacity: '0' })
  document.body.appendChild(ta)
  ta.select()
  let ok = false
  try {
    ok = document.execCommand('copy')
  } catch {
    ok = false
  }
  ta.remove()
  return ok
}
