// SettingsService.updateAppearanceSettings + utils/color.ts (light/dark/system, accent color).
const PAPERRUST_ACCENT = '#B7410E'

export function hexToRgb(hex: string) {
  const p = hex.replace('#', '').match(/.{1,2}/g) ?? ['0', '0', '0']
  return { r: parseInt(p[0], 16), g: parseInt(p[1], 16), b: parseInt(p[2], 16) }
}

export function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  r /= 255
  g /= 255
  b /= 255
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  let h = 0
  let s = 0
  const l = (max + min) / 2
  if (max !== min) {
    const d = max - min
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min)
    if (max === r) h = (g - b) / d + (g < b ? 6 : 0)
    else if (max === g) h = (b - r) / d + 2
    else h = (r - g) / d + 4
    h /= 6
  }
  return [h, s, l]
}

/** utils/color.ts estimateBrightnessForColor: true = light */
export function isLightColor(hex: string): boolean {
  const { r, g, b } = hexToRgb(hex)
  const lin = (v: number) => {
    v /= 255
    return v < 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
  }
  const lum = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
  return (lum + 0.05) * (lum + 0.05) > 0.15
}

export function applyAppearance(useSystem: boolean, darkEnabled: boolean, themeColor: string, doc: Document = document) {
  const html = doc.documentElement
  html.setAttribute('data-bs-theme', useSystem ? 'auto' : darkEnabled ? 'dark' : 'light')
  const body = doc.body
  if (themeColor?.length) {
    const [h, s, l] = rgbToHsl(...(Object.values(hexToRgb(themeColor)) as [number, number, number]))
    const light = isLightColor(themeColor)
    body.classList.toggle('primary-dark', !light)
    body.classList.toggle('primary-light', light)
    html.style.setProperty('--pr-primary', `${+h * 360},${s * 100}%`)
    html.style.setProperty('--pr-primary-lightness', `${l * 100}%`)
  } else {
    body.classList.remove('primary-dark', 'primary-light')
    html.style.removeProperty('--pr-primary')
    html.style.removeProperty('--pr-primary-lightness')
  }
  doc.querySelector('meta[name="theme-color"]')?.setAttribute('content', themeColor?.length ? themeColor : PAPERRUST_ACCENT)
}
