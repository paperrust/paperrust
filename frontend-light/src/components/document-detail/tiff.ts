// DocumentDetailComponent.tryRenderTiff: decode TIFF with UTIF (3.1.0 like Angular), largest page as PNG data URL.
// Loaded only for TIFF documents (own chunk incl. utif/pako).
import * as UTIFNs from 'utif'

const UTIF: any = (UTIFNs as any).default ?? UTIFNs

export function tiffToDataUrl(res: ArrayBuffer): string {
  // See UTIF.js > _imgLoaded
  const tiffIfds: any[] = UTIF.decode(res)
  let vsns = tiffIfds,
    ma = 0,
    page = vsns[0]
  if (tiffIfds[0].subIFD) vsns = vsns.concat(tiffIfds[0].subIFD)
  for (let i = 0; i < vsns.length; i++) {
    const img = vsns[i]
    if (img['t258'] == null || img['t258'].length < 3) continue
    const ar = img['t256'] * img['t257']
    if (ar > ma) {
      ma = ar
      page = img
    }
  }
  UTIF.decodeImage(res, page, tiffIfds)
  const rgba = UTIF.toRGBA8(page)
  const { width: w, height: h } = page
  const cnv = document.createElement('canvas')
  cnv.width = w
  cnv.height = h
  const ctx = cnv.getContext('2d')!
  const imgd = ctx.createImageData(w, h)
  for (let i = 0; i < rgba.length; i++) imgd.data[i] = rgba[i]
  ctx.putImageData(imgd, 0, 0)
  return cnv.toDataURL()
}
