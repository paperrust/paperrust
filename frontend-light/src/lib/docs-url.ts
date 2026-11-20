// paperrust.ts paperrustDocsUrl: upstream docs path (e.g. 'usage/#custom-fields') → PaperRust docs page
// (mapping from frontend/doc-links.json). Absolute under /rs/docs/, because the Light UI runs under /rs/app/.
const DOC_PAGES: Record<string, string> = {
  '': 'README.md',
  configuration: 'configuration.md',
  'configuration#*': 'configuration.md',
  usage: 'usage.md',
  'usage#document-trash': 'usage.md#papierkorb',
  'usage#users-and-groups': 'permissions.md',
  'usage#global-permissions': 'permissions.md',
  'usage#terms-and-definitions': 'usage.md',
  'usage#custom-fields': 'custom-fields.md',
  'usage#incoming-mail': 'mail.md',
  'usage#processed-mail': 'mail.md',
  'usage#workflows': 'workflows.md',
  advanced_usage: 'advanced.md',
  'advanced_usage#file-name-handling': 'storage-paths.md',
  'paperrust/share-links': 'usage.md#share-links-desktop',
}

export function docsUrl(upstreamPath?: string | null): string {
  const raw = (upstreamPath ?? '').trim()
  const hash = raw.indexOf('#')
  const path = (hash >= 0 ? raw.slice(0, hash) : raw).replace(/^\/+|\/+$/g, '')
  const anchor = hash >= 0 ? raw.slice(hash + 1) : ''
  let page: string
  if (anchor && DOC_PAGES[`${path}#${anchor}`] !== undefined) page = DOC_PAGES[`${path}#${anchor}`]
  else if (anchor && DOC_PAGES[`${path}#*`] !== undefined) {
    page = DOC_PAGES[`${path}#*`]
    if (!page.includes('#')) page += '#' + anchor.toLowerCase()
  } else if (DOC_PAGES[path] !== undefined) page = DOC_PAGES[path]
  else page = DOC_PAGES[''] ?? ''
  return '/rs/docs/' + page
}
