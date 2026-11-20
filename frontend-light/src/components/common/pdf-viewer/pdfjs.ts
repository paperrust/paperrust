// pdf.js as in Angular (common/pdf-viewer): legacy build + web/pdf_viewer.mjs, same version (package.json).
// Loaded only on demand (own chunks). pdf_viewer.mjs reads globalThis.pdfjsLib → load one after the other.
import workerUrl from 'pdfjs-dist/legacy/build/pdf.worker.min.mjs?url'

export type PdfjsLib = typeof import('pdfjs-dist/legacy/build/pdf.mjs')
export type PdfjsViewer = typeof import('pdfjs-dist/web/pdf_viewer.mjs')

let loading: Promise<{ lib: PdfjsLib; viewer: PdfjsViewer }> | undefined

export function loadPdfjs() {
  loading ??= (async () => {
    const lib = await import('pdfjs-dist/legacy/build/pdf.mjs')
    const viewer = await import('pdfjs-dist/web/pdf_viewer.mjs')
    lib.GlobalWorkerOptions.workerSrc = new URL(workerUrl, document.baseURI).toString()
    return { lib, viewer }
  })()
  return loading
}

/** Location of the pdf.js helper files (vite.config.ts copies them to dist/assets/pdfjs/) */
export const pdfjsAssetUrl = (dir: 'wasm' | 'iccs' | 'cmaps') => new URL(`${import.meta.env.BASE_URL}assets/pdfjs/${dir}/`, document.baseURI).toString()

export enum PdfRenderMode {
  Single = 'single',
  All = 'all',
}

export enum PdfZoomScale {
  PageFit = 'page-fit',
  PageWidth = 'page-width',
}

export enum PdfZoomLevel {
  Quarter = '.25',
  Half = '.5',
  ThreeQuarters = '.75',
  One = '1',
  OneAndHalf = '1.5',
  Two = '2',
  Three = '3',
}
