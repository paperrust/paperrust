import { defineConfig } from 'vitest/config'
import type { Plugin } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { pdfjsAssets } from './tools/pdfjs-assets.mjs'
import { scopedStyles } from './tools/ng-scope.mjs'
import { bootstrapCss } from './tools/bootstrap-css.mjs'

// Light UI under /rs/app/ (docs/ARCHITECTURE-V2.md). Until pl-server mounts /rs/app/ (package P4),
// `vite preview` serves the Light UI and forwards everything else (API, login, Angular under /)
// to the local PaperRust test server (PAPERRUST_URL, default 127.0.0.1:8330).
const target = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.PAPERRUST_URL ?? 'http://127.0.0.1:8330'
const proxy = {
  '^/(?!rs/app(/|$)).*': { target, changeOrigin: false, ws: true },
}

// Second UI (docs/DESIGN2.md): own entry m.html under /rs/app/m/… – like pl-server
// (light_ui.rs spa_entry) also in `vite dev`/`vite preview`.
const modernEntry = (): Plugin => {
  const rewrite = (req: { url?: string }, _res: unknown, next: () => void) => {
    if (req.url && /^\/rs\/app\/m(\/|\?|$)/.test(req.url) && !/\.[a-z0-9]+(\?|$)/i.test(req.url.split('?')[0])) req.url = '/rs/app/m.html'
    next()
  }
  return {
    name: 'paperrust-modern-entry',
    configureServer: (s) => void s.middlewares.use(rewrite),
    configurePreviewServer: (s) => void s.middlewares.use(rewrite),
  }
}

export default defineConfig({
  base: '/rs/app/',
  plugins: [modernEntry(), bootstrapCss(), scopedStyles(), svelte(), pdfjsAssets()],
  build: {
    target: 'es2022',
    modulePreload: { polyfill: false },
    manifest: true,
    reportCompressedSize: false,
    assetsInlineLimit: 0,
    rollupOptions: { input: { index: 'index.html', m: 'm.html' } },
  },
  server: { proxy },
  preview: { proxy },
  test: {
    include: ['src/**/*.test.ts', 'tools/**/*.test.mjs'],
    setupFiles: ['src/test-setup.ts'],
    environment: 'node',
  },
})
