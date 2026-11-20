// Tests of the parity tools: Angular encapsulation, whitespace rules, settings access, page list.
import { describe, expect, it } from 'vitest'
import { scopeSelector, scopeCss, addMarkupAttrs, scopedStyleName } from '../../tools/ng-scope.mjs'
import { pruneCss } from '../../tools/bootstrap-css.mjs'
import { transformMarkup } from '../../tools/ng-markup.js'
import { getSetting, rawValue, SETTINGS_KEYS, toBool } from '../stores/settings.svelte'
import { paginationPages } from '../components/common/pagination/pages'

describe('Kapselung der Baustein-Styles (ng-scope)', () => {
  it('jede Verbundgruppe bekommt das Inhalts-Attribut', () => {
    expect(scopeSelector('.sidebar .nav-link', 'app-frame')).toEqual(['.sidebar[sc-app-frame] .nav-link[sc-app-frame]'])
    expect(scopeSelector('h3 > .h6', 'x')).toEqual(['h3[sc-x] > .h6[sc-x]'])
    expect(scopeSelector('a:hover::after', 'x')).toEqual(['a[sc-x]:hover::after'])
  })
  it(':host, :host(), :host-context()', () => {
    expect(scopeSelector(':host', 'tag')).toEqual(['[sh-tag]'])
    expect(scopeSelector(':host(.big) .a', 'tag')).toEqual(['.big[sh-tag] .a[sc-tag]'])
    expect(scopeSelector(':host-context(.primary-light) .navbar-brand', 'f')).toEqual([
      '.primary-light[sh-f] .navbar-brand[sc-f]',
      '.primary-light [sh-f] .navbar-brand[sc-f]',
    ])
  })
  it('::deep beendet die Kapselung', () => {
    expect(scopeSelector('::deep .popover-slim .popover-body', 'f')).toEqual(['.popover-slim .popover-body'])
    expect(scopeSelector(':host ::deep .navbar-action', 'f')).toEqual(['[sh-f] .navbar-action'])
    expect(scopeSelector('.navbar-brand ::deep .logo .leaf', 'f')).toEqual(['.navbar-brand[sc-f] .logo .leaf'])
  })
  it('scopeCss lässt @keyframes aus und kapselt in @media', () => {
    const css = scopeCss('@media (min-width: 1px){.a{x:1}} @keyframes k{from{o:0}to{o:1}}', 'n')
    expect(css).toContain('.a[sc-n]')
    expect(css).toContain('from{o:0}')
  })
  it('Markup-Attribute: sc an Elementen, sh an Host-Tags', () => {
    expect(addMarkupAttrs('<div class="a">', 'list')).toBe('<div sc-list class="a">')
    expect(addMarkupAttrs('<pngx-tag class="ms-1">', 'list')).toBe('<pngx-tag sc-list sh-tag class="ms-1">')
    expect(addMarkupAttrs('<Tag tag={x} />', 'list')).toBe('<Tag tag={x} />')
    expect(addMarkupAttrs('</div>', 'list')).toBe('</div>')
  })
})

describe('Eigene Stylesheets beim Bauen', () => {
  it('nur src/styles/components/<name>.css wird gekapselt', () => {
    expect(scopedStyleName('/x/src/styles/components/app-frame.css')).toBe('app-frame')
    expect(scopedStyleName(String.raw`C:\x\src\styles\components\input-tags.css?direct`)).toBe('input-tags')
    expect(scopedStyleName('/x/src/styles/base.scss')).toBeNull()
  })
  it('Bootstrap kürzen: unbekannte Klassen raus, :not(…) und Elementregeln bleiben', () => {
    const css = '.btn{a:1}.carousel{b:2}.btn:not(.btn-check){c:3}body{d:4}.show{e:5}@media (min-width:1px){.carousel-x{f:6}}'
    const out = pruneCss(css, new Set(['btn'])).css
    expect(out).toContain('.btn{a:1}')
    expect(out).toContain('.btn:not(.btn-check)')
    expect(out).toContain('body{d:4}')
    expect(out).toContain('.show{e:5}')
    expect(out).not.toContain('carousel')
    expect(out).not.toContain('@media')
  })
})

describe('Whitespace wie Angular (ng-markup)', () => {
  it('reiner Whitespace zwischen Tags/Blöcken entfällt', () => {
    expect(transformMarkup('<div>\n  <span>a</span>\n  <span>b</span>\n</div>')).toBe('<div><span>a</span><span>b</span></div>')
    expect(transformMarkup('<ul>\n {#if x}\n  <li>1</li>\n {/if}\n</ul>')).toBe('<ul>{#if x}<li>1</li>{/if}</ul>')
  })
  it('Rand-Whitespace von Text bleibt als ein Leerzeichen', () => {
    expect(transformMarkup('<span class="small">\n  {name}\n</span>')).toBe("<span class=\"small\">{' '}{name}{' '}</span>")
    expect(transformMarkup('<b>x</b> und <i>y</i>')).toBe("<b>x</b>{' '}und{' '}<i>y</i>")
  })
  it('Ausdrücke mit Klammern/Strings werden nicht zerlegt', () => {
    const s = "<p>{t('a {$X} b', { X: '}' })}</p>"
    expect(transformMarkup(s)).toBe(s)
  })
})

describe('UI-Settings wie SettingsService', () => {
  const s = { saved_views: { warn_on_unsaved_change: true }, dark_mode: { use_system: true, enabled: 'false' }, documentListSize: 25, language: 'de-DE' }
  it('Schlüsselpfad general-settings:a-b:c → a_b.c, Typen und Standardwerte', () => {
    expect(rawValue(s, SETTINGS_KEYS.DARK_MODE_ENABLED)).toBe('false')
    expect(getSetting(s, SETTINGS_KEYS.DARK_MODE_ENABLED)).toBe(false)
    expect(getSetting(s, SETTINGS_KEYS.DARK_MODE_USE_SYSTEM)).toBe(true)
    // A-279/A-280: strings and unknown values handled robustly (no JSON.parse crash)
    expect(getSetting({ dark_mode: { use_system: 'False', enabled: 'TRUE' } }, SETTINGS_KEYS.DARK_MODE_USE_SYSTEM)).toBe(false)
    expect(getSetting({ dark_mode: { enabled: 'TRUE' } }, SETTINGS_KEYS.DARK_MODE_ENABLED)).toBe(true)
    expect(getSetting({ dark_mode: { use_system: 'kaputt' } }, SETTINGS_KEYS.DARK_MODE_USE_SYSTEM)).toBe(true)
    expect([toBool(0, true), toBool(1, false), toBool('', true), toBool({}, false)]).toEqual([false, true, false, false])
    expect(getSetting(s, SETTINGS_KEYS.DOCUMENT_LIST_SIZE)).toBe(25)
    expect(getSetting(s, SETTINGS_KEYS.SIDEBAR_VIEWS_VISIBLE_IDS)).toEqual([])
    expect(getSetting(s, SETTINGS_KEYS.NOTES_ENABLED)).toBe(true)
    expect(getSetting(s, SETTINGS_KEYS.LANGUAGE)).toBe('de-DE')
    expect(getSetting({ __proto__: { x: 1 } }, SETTINGS_KEYS.LANGUAGE)).toBe('')
  })
})

describe('Seitenliste wie NgbPagination (maxSize 5, rotate, ellipses)', () => {
  it.each([
    [1, 4, [1, 2, 3, 4]],
    [1, 20, [1, 2, 3, 4, 5, -1, 20]],
    [10, 20, [1, -1, 8, 9, 10, 11, 12, -1, 20]],
    [20, 20, [1, -1, 16, 17, 18, 19, 20]],
    [4, 7, [1, 2, 3, 4, 5, 6, 7]],
  ])('Seite %i von %i', (page, count, expected) => {
    expect(paginationPages(page, count, 5, true, true)).toEqual(expected)
  })
})
