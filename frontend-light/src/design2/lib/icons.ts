// Icons of the second UI (bootstrap-icons via ?raw, only these end up in the bundle). Name → SVG.
// Deliberately ico() instead of icon(): tools/icons.mjs counts icon('…') for the classic UI.
import i0 from 'bootstrap-icons/icons/grid-1x2.svg?raw'
import i1 from 'bootstrap-icons/icons/inbox.svg?raw'
import i2 from 'bootstrap-icons/icons/files.svg?raw'
import i3 from 'bootstrap-icons/icons/bookmark.svg?raw'
import i5 from 'bootstrap-icons/icons/funnel.svg?raw'
import i6 from 'bootstrap-icons/icons/stars.svg?raw'
import i7 from 'bootstrap-icons/icons/sliders.svg?raw'
import i8 from 'bootstrap-icons/icons/gear.svg?raw'
import i9 from 'bootstrap-icons/icons/search.svg?raw'
import i10 from 'bootstrap-icons/icons/cloud-arrow-up.svg?raw'
import i11 from 'bootstrap-icons/icons/bell.svg?raw'
import i12 from 'bootstrap-icons/icons/person-circle.svg?raw'
import i13 from 'bootstrap-icons/icons/sun.svg?raw'
import i14 from 'bootstrap-icons/icons/moon.svg?raw'
import i15 from 'bootstrap-icons/icons/circle-half.svg?raw'
import i16 from 'bootstrap-icons/icons/x-lg.svg?raw'
import i17 from 'bootstrap-icons/icons/check-lg.svg?raw'
import i18 from 'bootstrap-icons/icons/check2.svg?raw'
import i19 from 'bootstrap-icons/icons/chevron-down.svg?raw'
import i20 from 'bootstrap-icons/icons/chevron-left.svg?raw'
import i21 from 'bootstrap-icons/icons/chevron-right.svg?raw'
import i22 from 'bootstrap-icons/icons/trash.svg?raw'
import i23 from 'bootstrap-icons/icons/download.svg?raw'
import i24 from 'bootstrap-icons/icons/tag.svg?raw'
import i25 from 'bootstrap-icons/icons/tags.svg?raw'
import i26 from 'bootstrap-icons/icons/person.svg?raw'
import i27 from 'bootstrap-icons/icons/file-earmark.svg?raw'
import i28 from 'bootstrap-icons/icons/folder.svg?raw'
import i29 from 'bootstrap-icons/icons/plus-lg.svg?raw'
import i30 from 'bootstrap-icons/icons/pencil.svg?raw'
import i31 from 'bootstrap-icons/icons/three-dots.svg?raw'
import i32 from 'bootstrap-icons/icons/box-arrow-up-right.svg?raw'
import i33 from 'bootstrap-icons/icons/clock-history.svg?raw'
import i34 from 'bootstrap-icons/icons/journal-text.svg?raw'
import i35 from 'bootstrap-icons/icons/card-text.svg?raw'
import i36 from 'bootstrap-icons/icons/list.svg?raw'
import i37 from 'bootstrap-icons/icons/keyboard.svg?raw'
import i38 from 'bootstrap-icons/icons/calendar-event.svg?raw'
import i39 from 'bootstrap-icons/icons/hourglass-split.svg?raw'
import i40 from 'bootstrap-icons/icons/exclamation-triangle.svg?raw'
import i41 from 'bootstrap-icons/icons/send.svg?raw'
import i42 from 'bootstrap-icons/icons/arrow-clockwise.svg?raw'
import i43 from 'bootstrap-icons/icons/sort-down.svg?raw'
import i44 from 'bootstrap-icons/icons/sort-up.svg?raw'
import i45 from 'bootstrap-icons/icons/door-open.svg?raw'
import i46 from 'bootstrap-icons/icons/question-circle.svg?raw'
import i47 from 'bootstrap-icons/icons/file-earmark-arrow-up.svg?raw'
import i48 from 'bootstrap-icons/icons/zoom-in.svg?raw'
import i49 from 'bootstrap-icons/icons/zoom-out.svg?raw'
import i50 from 'bootstrap-icons/icons/people.svg?raw'
import i51 from 'bootstrap-icons/icons/archive.svg?raw'
import i53 from 'bootstrap-icons/icons/x-circle.svg?raw'
import i54 from 'bootstrap-icons/icons/info-circle.svg?raw'
import i55 from 'bootstrap-icons/icons/check-circle.svg?raw'
import i56 from 'bootstrap-icons/icons/stopwatch.svg?raw'
import i57 from 'bootstrap-icons/icons/shield-check.svg?raw'
import i58 from 'bootstrap-icons/icons/calendar3.svg?raw'
import i59 from 'bootstrap-icons/icons/arrow-left.svg?raw'
import i60 from 'bootstrap-icons/icons/square.svg?raw'
import i61 from 'bootstrap-icons/icons/check-square.svg?raw'
import i62 from 'bootstrap-icons/icons/dash-square.svg?raw'
import i64 from 'bootstrap-icons/icons/chat-square-text.svg?raw'
import i65 from 'bootstrap-icons/icons/layout-three-columns.svg?raw'
import i66 from 'bootstrap-icons/icons/file-earmark-text.svg?raw'
import i67 from 'bootstrap-icons/icons/upc-scan.svg?raw'
import i68 from 'bootstrap-icons/icons/diagram-3.svg?raw'
import i69 from 'bootstrap-icons/icons/envelope.svg?raw'
import i70 from 'bootstrap-icons/icons/person-badge.svg?raw'
import i71 from 'bootstrap-icons/icons/list-task.svg?raw'
import i72 from 'bootstrap-icons/icons/layout-sidebar.svg?raw'
import i73 from 'bootstrap-icons/icons/eye.svg?raw'
import i74 from 'bootstrap-icons/icons/gear-wide-connected.svg?raw'
import i75 from 'bootstrap-icons/icons/chat-left-text.svg?raw'
import i76 from 'bootstrap-icons/icons/printer.svg?raw'
import i77 from 'bootstrap-icons/icons/hdd-stack.svg?raw'
import i78 from 'bootstrap-icons/icons/link-45deg.svg?raw'
// PDF tools in the workspace (pdf-actions.ts)
import i79 from 'bootstrap-icons/icons/file-earmark-break.svg?raw'
import i80 from 'bootstrap-icons/icons/eraser.svg?raw'
import i81 from 'bootstrap-icons/icons/arrows-angle-contract.svg?raw'
import i82 from 'bootstrap-icons/icons/file-earmark-pdf.svg?raw'
import i83 from 'bootstrap-icons/icons/arrow-counterclockwise.svg?raw'
import i84 from 'bootstrap-icons/icons/arrow-repeat.svg?raw'
import i85 from 'bootstrap-icons/icons/union.svg?raw'
import i86 from 'bootstrap-icons/icons/layers.svg?raw'
import i87 from 'bootstrap-icons/icons/signpost.svg?raw'

const SET: Record<string, string> = {
  'signpost': i87,
  'file-earmark-break': i79,
  'eraser': i80,
  'arrows-angle-contract': i81,
  'file-earmark-pdf': i82,
  'arrow-counterclockwise': i83,
  'arrow-repeat': i84,
  'union': i85,
  'layers': i86,
  printer: i76,
  'hdd-stack': i77,
  'link-45deg': i78,
  'grid-1x2': i0,
  'inbox': i1,
  'files': i2,
  'bookmark': i3,
  'funnel': i5,
  'stars': i6,
  'sliders': i7,
  'gear': i8,
  'search': i9,
  'cloud-arrow-up': i10,
  'bell': i11,
  'person-circle': i12,
  'sun': i13,
  'moon': i14,
  'circle-half': i15,
  'x-lg': i16,
  'check-lg': i17,
  'check2': i18,
  'chevron-down': i19,
  'chevron-left': i20,
  'chevron-right': i21,
  'trash': i22,
  'download': i23,
  'tag': i24,
  'tags': i25,
  'person': i26,
  'file-earmark': i27,
  'folder': i28,
  'plus-lg': i29,
  'pencil': i30,
  'three-dots': i31,
  'box-arrow-up-right': i32,
  'clock-history': i33,
  'journal-text': i34,
  'card-text': i35,
  'list': i36,
  'keyboard': i37,
  'calendar-event': i38,
  'hourglass-split': i39,
  'exclamation-triangle': i40,
  'send': i41,
  'arrow-clockwise': i42,
  'sort-down': i43,
  'sort-up': i44,
  'door-open': i45,
  'question-circle': i46,
  'file-earmark-arrow-up': i47,
  'zoom-in': i48,
  'zoom-out': i49,
  'people': i50,
  'archive': i51,
  'x-circle': i53,
  'info-circle': i54,
  'check-circle': i55,
  'stopwatch': i56,
  'shield-check': i57,
  'calendar3': i58,
  'arrow-left': i59,
  'square': i60,
  'check-square': i61,
  'dash-square': i62,
  'chat-square-text': i64,
  'layout-three-columns': i65,
  'file-earmark-text': i66,
  'upc-scan': i67,
  'diagram-3': i68,
  'envelope': i69,
  'person-badge': i70,
  'list-task': i71,
  'layout-sidebar': i72,
  'eye': i73,
  'gear-wide-connected': i74,
  'chat-left-text': i75,
}

export type IconName = keyof typeof SET

/**
 * Fixed mapping meaning → icon (docs/DESIGN2-ENTWIRREN.md §7: "every icon must fit"). New places
 * take the icon from here, so that the same thing looks the same everywhere and no icon means two things.
 * "stars" (sparkle) only for AI/assistant.
 */
export const SYMBOL = {
  overview: 'grid-1x2',
  inbox: 'inbox',
  documents: 'files',
  document: 'file-earmark',
  tag: 'tag',
  tags: 'tags',
  correspondent: 'person',
  documentType: 'file-earmark-text',
  storagePath: 'folder',
  customField: 'card-text',
  savedView: 'bookmark',
  asnLabels: 'upc-scan',
  scan: 'printer',
  shareLinks: 'link-45deg',
  scanners: 'hdd-stack',
  protection: 'shield-check',
  usersGroups: 'people',
  roles: 'person-badge',
  workflow: 'diagram-3',
  mail: 'envelope',
  tasks: 'list-task',
  logs: 'journal-text',
  trash: 'trash',
  management: 'sliders',
  configuration: 'gear-wide-connected',
  settings: 'gear',
  ai: 'stars',
  conversation: 'chat-left-text',
  notes: 'chat-square-text',
  visibility: 'eye',
  send: 'send',
  upload: 'cloud-arrow-up',
  notifications: 'bell',
  menu: 'list',
  designPaperRust: 'layout-sidebar',
  designClassic: 'layout-three-columns',
  shortcuts: 'keyboard',
  date: 'calendar-event',
} as const satisfies Record<string, string>

/** Inline SVG without fixed size (size via .d2-ico / font-size), decorative (aria-hidden). */
export function ico(name: string): string {
  const svg = SET[name] ?? ''
  return svg.replace(/ width="16" height="16"/, '').replace('<svg ', '<svg aria-hidden="true" focusable="false" ')
}

export const iconNames = () => Object.keys(SET)

/** Register further icons of a lazily loaded area (stay out of the first call, e.g. Automation) */
export function addIcons(more: Record<string, string>) {
  for (const [k, v] of Object.entries(more)) if (!SET[k]) SET[k] = v
}
