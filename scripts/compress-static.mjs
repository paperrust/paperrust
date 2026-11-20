#!/usr/bin/env node
// Place precompressed .gz/.br next to every file – like whitenoise 6 (compress.py): leave out known
// binary formats, keep the result only if it is at most 95 % of the original size.
//   node scripts/compress-static.mjs <dir>
// Parallel in worker threads (Brotli level 11 is expensive: serial on a 4-core ARM ~17 min).
// COMPRESS_JOBS limits the parallelism (default: number of cores, at most 8). Output
// byte-identical to the serial version (same parameters, deterministic compressors).
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import zlib from 'node:zlib'
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads'

function compress(p) {
  const buf = fs.readFileSync(p)
  if (!buf.length) return 0
  let n = 0
  const gz = zlib.gzipSync(buf, { level: 9 })
  if (gz.length <= buf.length * 0.95) fs.writeFileSync(p + '.gz', gz), n++
  const br = zlib.brotliCompressSync(buf, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: buf.length },
  })
  if (br.length <= buf.length * 0.95) fs.writeFileSync(p + '.br', br), n++
  return n
}

if (!isMainThread) {
  // each worker takes every JOBS-th file from its index
  const { files, start, step } = workerData
  let n = 0
  for (let i = start; i < files.length; i += step) n += compress(files[i])
  parentPort.postMessage(n)
} else {
  const SKIP = new Set(
    'jpg jpeg png gif webp zip gz tgz bz2 tbz xz br swf flv woff woff2 3gp 3gpp asf avi m4v mov mp4 mpeg mpg webm wmv'.split(' ')
  )
  const dir = process.argv[2]
  if (!dir) {
    console.error('usage: compress.mjs <dir>')
    process.exit(2)
  }
  // first collect all files (the new .gz/.br are not compressed themselves)
  const files = []
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name)
      if (e.isDirectory()) walk(p)
      else if (!SKIP.has(path.extname(e.name).slice(1).toLowerCase())) files.push(p)
    }
  }
  walk(dir)
  const cores = os.availableParallelism ? os.availableParallelism() : os.cpus().length
  const step = Math.max(1, Math.min(Number(process.env.COMPRESS_JOBS) || Math.min(cores, 8), files.length || 1))
  const counts = await Promise.all(
    Array.from(
      { length: step },
      (_, start) =>
        new Promise((resolve, reject) => {
          const w = new Worker(new URL(import.meta.url), { workerData: { files, start, step } })
          w.once('message', resolve)
          w.once('error', reject)
          w.once('exit', (code) => code !== 0 && reject(new Error(`Worker beendet mit ${code}`)))
        })
    )
  )
  console.log(`[static] ${counts.reduce((a, b) => a + b, 0)} vorkomprimierte Dateien (.gz/.br)`)
}
