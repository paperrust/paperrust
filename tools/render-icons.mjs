#!/usr/bin/env node
// PaperRust: generate raster icons of the frontend from docs/design/assets (once or after
// logo changes; the result is versioned in docs/design/assets/png/ or
// scripts/static-assets/favicon.ico, the build needs no browser).
//
//   PLAYWRIGHT_BROWSERS_PATH=tests/e2e/.browsers node tools/render-icons.mjs
//
// Chromium (Playwright from tests/e2e) renders the SVGs incl. filters (feTurbulence):
//   apple-touch-icon.png 180, icon-192.png, icon-512.png  <- paperrust-app-icon.svg
//   icon-512-maskable.png                                 <- app icon full-bleed (safe zone 80 %)
//   favicon.ico (16/32/48/256, PNG entries)              <- favicon.svg (16-48) / app icon (256)
import fs from 'node:fs'
import path from 'node:path'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const REPO = path.resolve(HERE, '..')
const ASSETS = path.join(REPO, 'docs/design/assets')
const OUT = path.join(REPO, 'docs/design/assets/png')
const ICO_OUT = path.join(REPO, 'scripts/static-assets')
const req = createRequire(path.join(REPO, 'tests/e2e/package.json'))
const { chromium } = req('playwright')

const appIcon = fs.readFileSync(path.join(ASSETS, 'paperrust-app-icon.svg'), 'utf8')
const favicon = fs.readFileSync(path.join(ASSETS, 'favicon.svg'), 'utf8')
// maskable: background full-bleed in iron, content at 80 % (Android crops to a circle at most)
const maskable = appIcon
  .replace('<rect x="16" y="16" width="480" height="480" rx="112"', '<rect x="0" y="0" width="512" height="512" rx="0"')
  .replace(/<path d="M150/, '<g transform="translate(51.2 51.2) scale(0.8)"><path d="M150')
  .replace(/<\/svg>$/, '</g></svg>')

fs.mkdirSync(OUT, { recursive: true })
const browser = await chromium.launch()
const page = await browser.newPage({ deviceScaleFactor: 1 })
async function render(svg, size) {
  await page.setViewportSize({ width: size, height: size })
  const s = svg.replace('<svg ', `<svg width="${size}" height="${size}" `)
  await page.setContent(
    `<!doctype html><html><body style="margin:0;background:transparent">${s}</body></html>`
  )
  return page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } })
}

const files = {
  'apple-touch-icon.png': await render(appIcon, 180),
  'icon-192.png': await render(appIcon, 192),
  'icon-512.png': await render(appIcon, 512),
  'icon-512-maskable.png': await render(maskable, 512),
}
for (const [n, b] of Object.entries(files)) fs.writeFileSync(path.join(OUT, n), b)

// ICO with PNG entries (Vista+, supported by all browsers)
const icoImgs = [
  [16, await render(favicon, 16)],
  [32, await render(favicon, 32)],
  [48, await render(favicon, 48)],
  [256, await render(appIcon, 256)],
]
const head = Buffer.alloc(6 + 16 * icoImgs.length)
head.writeUInt16LE(0, 0)
head.writeUInt16LE(1, 2)
head.writeUInt16LE(icoImgs.length, 4)
let off = head.length
icoImgs.forEach(([size, png], i) => {
  const e = 6 + 16 * i
  head.writeUInt8(size >= 256 ? 0 : size, e)
  head.writeUInt8(size >= 256 ? 0 : size, e + 1)
  head.writeUInt8(0, e + 2)
  head.writeUInt8(0, e + 3)
  head.writeUInt16LE(1, e + 4)
  head.writeUInt16LE(32, e + 6)
  head.writeUInt32LE(png.length, e + 8)
  head.writeUInt32LE(off, e + 12)
  off += png.length
})
fs.writeFileSync(path.join(ICO_OUT, 'favicon.ico'), Buffer.concat([head, ...icoImgs.map((x) => x[1])]))
await browser.close()
console.log('render-icons: wrote', Object.keys(files).concat('favicon.ico').join(', '), 'to', OUT, '+', ICO_OUT)
