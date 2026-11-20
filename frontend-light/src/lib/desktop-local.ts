// PaperRust Desktop: does the UI run in the local window of the desktop app?
//
// Only the desktop supervisor sets PAPERRUST_SHARE_MODE (local|lan); pl-server then writes
// <meta name="paperrust_share_mode"> into the entry pages. Together with a loopback address this means:
// local window with automatic login (docs/DESKTOP.md §25). "Log out" makes no sense there
// – the app immediately logs in again (owner finding 2026-11-20) –, so it is hidden.
// Access from other devices (LAN) and server operation keep "Log out".

const LOOPBACK = new Set(['127.0.0.1', 'localhost', '[::1]', '::1'])

export function isLocalDesktopWindow(doc: Document = document, host: string = location.hostname): boolean {
  const mode = doc.querySelector('meta[name="paperrust_share_mode"]')?.getAttribute('content') ?? ''
  return (mode === 'local' || mode === 'lan') && LOOPBACK.has(host)
}
