#!/usr/bin/env node
// Precompressed .br/.gz for dist/ (like static.files in pl-server: br/gz next to the file).
import fs from 'node:fs'
import path from 'node:path'
import zlib from 'node:zlib'
import { fileURLToPath } from 'node:url'

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const DIST = path.join(ROOT, 'dist')

export const brotli = (buf) =>
  zlib.brotliCompressSync(buf, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: buf.length } })
export const gzip = (buf) => zlib.gzipSync(buf, { level: 9 })

function walk(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else out.push(p)
  }
  return out
}

function main() {
  let n = 0
  for (const f of walk(DIST)) {
    if (/\.(br|gz)$/.test(f) || !/\.(js|mjs|css|html|json|svg|txt|webmanifest)$/.test(f)) continue
    const buf = fs.readFileSync(f)
    fs.writeFileSync(f + '.br', brotli(buf))
    fs.writeFileSync(f + '.gz', gzip(buf))
    n++
  }
  console.log(`[compress] ${n} Dateien → .br/.gz`)
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
