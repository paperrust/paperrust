import fs from 'node:fs'
import path from 'node:path'

// Ship the pdf.js helper files (wasm decoder, ICC profiles, CMaps) like Angular (angular.json assets):
// dist/assets/pdfjs/{wasm,iccs,cmaps}/ – loaded by the pdf.js worker only on demand (document view).
export function pdfjsAssets() {
  const root = path.resolve('node_modules/pdfjs-dist')
  return {
    name: 'pdfjs-assets',
    generateBundle() {
      for (const dir of ['wasm', 'iccs', 'cmaps']) {
        for (const f of fs.readdirSync(path.join(root, dir))) {
          this.emitFile({ type: 'asset', fileName: `assets/pdfjs/${dir}/${f}`, source: fs.readFileSync(path.join(root, dir, f)) })
        }
      }
    },
  }
}
