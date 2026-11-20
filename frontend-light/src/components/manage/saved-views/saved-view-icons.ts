// Port of data/saved-view-icons.ts (SAVED_VIEW_ICONS, DEFAULT_SAVED_VIEW_ICON). The icons are only loaded with this
// route (lib/route-icons.ts) instead of in the first call.
import { msg } from '../../../lib/i18n'
import { registerIcons } from '../../../lib/route-icons'
import i0 from 'bootstrap-icons/icons/funnel.svg?raw'
import i1 from 'bootstrap-icons/icons/archive.svg?raw'
import i2 from 'bootstrap-icons/icons/bank.svg?raw'
import i3 from 'bootstrap-icons/icons/basket.svg?raw'
import i4 from 'bootstrap-icons/icons/bell.svg?raw'
import i5 from 'bootstrap-icons/icons/bookmark.svg?raw'
import i6 from 'bootstrap-icons/icons/boxes.svg?raw'
import i7 from 'bootstrap-icons/icons/briefcase.svg?raw'
import i8 from 'bootstrap-icons/icons/building.svg?raw'
import i9 from 'bootstrap-icons/icons/calculator.svg?raw'
import i10 from 'bootstrap-icons/icons/calendar.svg?raw'
import i11 from 'bootstrap-icons/icons/camera.svg?raw'
import i12 from 'bootstrap-icons/icons/card-checklist.svg?raw'
import i13 from 'bootstrap-icons/icons/cash.svg?raw'
import i14 from 'bootstrap-icons/icons/chat-left-text.svg?raw'
import i15 from 'bootstrap-icons/icons/check-circle.svg?raw'
import i16 from 'bootstrap-icons/icons/clipboard.svg?raw'
import i17 from 'bootstrap-icons/icons/clock-history.svg?raw'
import i18 from 'bootstrap-icons/icons/credit-card.svg?raw'
import i19 from 'bootstrap-icons/icons/download.svg?raw'
import i20 from 'bootstrap-icons/icons/envelope.svg?raw'
import i21 from 'bootstrap-icons/icons/exclamation-triangle.svg?raw'
import i22 from 'bootstrap-icons/icons/file-earmark.svg?raw'
import i23 from 'bootstrap-icons/icons/file-earmark-check.svg?raw'
import i24 from 'bootstrap-icons/icons/file-earmark-lock.svg?raw'
import i25 from 'bootstrap-icons/icons/file-earmark-medical.svg?raw'
import i26 from 'bootstrap-icons/icons/file-earmark-person.svg?raw'
import i27 from 'bootstrap-icons/icons/file-earmark-spreadsheet.svg?raw'
import i28 from 'bootstrap-icons/icons/file-text.svg?raw'
import i29 from 'bootstrap-icons/icons/files.svg?raw'
import i30 from 'bootstrap-icons/icons/folder.svg?raw'
import i31 from 'bootstrap-icons/icons/gear.svg?raw'
import i32 from 'bootstrap-icons/icons/globe2.svg?raw'
import i33 from 'bootstrap-icons/icons/hash.svg?raw'
import i34 from 'bootstrap-icons/icons/heart.svg?raw'
import i35 from 'bootstrap-icons/icons/house.svg?raw'
import i36 from 'bootstrap-icons/icons/inbox.svg?raw'
import i37 from 'bootstrap-icons/icons/journals.svg?raw'
import i38 from 'bootstrap-icons/icons/list-task.svg?raw'
import i39 from 'bootstrap-icons/icons/newspaper.svg?raw'
import i40 from 'bootstrap-icons/icons/paperclip.svg?raw'
import i41 from 'bootstrap-icons/icons/people.svg?raw'
import i42 from 'bootstrap-icons/icons/person.svg?raw'
import i43 from 'bootstrap-icons/icons/printer.svg?raw'
import i44 from 'bootstrap-icons/icons/receipt.svg?raw'
import i45 from 'bootstrap-icons/icons/safe.svg?raw'
import i46 from 'bootstrap-icons/icons/search.svg?raw'
import i47 from 'bootstrap-icons/icons/send.svg?raw'
import i48 from 'bootstrap-icons/icons/shop.svg?raw'
import i49 from 'bootstrap-icons/icons/stack.svg?raw'
import i50 from 'bootstrap-icons/icons/stars.svg?raw'
import i51 from 'bootstrap-icons/icons/tag.svg?raw'
import i52 from 'bootstrap-icons/icons/tags.svg?raw'
import i53 from 'bootstrap-icons/icons/telephone.svg?raw'
import i54 from 'bootstrap-icons/icons/truck.svg?raw'
import i55 from 'bootstrap-icons/icons/upc-scan.svg?raw'
import i56 from 'bootstrap-icons/icons/wallet2.svg?raw'

registerIcons({
  "funnel": i0,
  "archive": i1,
  "bank": i2,
  "basket": i3,
  "bell": i4,
  "bookmark": i5,
  "boxes": i6,
  "briefcase": i7,
  "building": i8,
  "calculator": i9,
  "calendar": i10,
  "camera": i11,
  "card-checklist": i12,
  "cash": i13,
  "chat-left-text": i14,
  "check-circle": i15,
  "clipboard": i16,
  "clock-history": i17,
  "credit-card": i18,
  "download": i19,
  "envelope": i20,
  "exclamation-triangle": i21,
  "file-earmark": i22,
  "file-earmark-check": i23,
  "file-earmark-lock": i24,
  "file-earmark-medical": i25,
  "file-earmark-person": i26,
  "file-earmark-spreadsheet": i27,
  "file-text": i28,
  "files": i29,
  "folder": i30,
  "gear": i31,
  "globe2": i32,
  "hash": i33,
  "heart": i34,
  "house": i35,
  "inbox": i36,
  "journals": i37,
  "list-task": i38,
  "newspaper": i39,
  "paperclip": i40,
  "people": i41,
  "person": i42,
  "printer": i43,
  "receipt": i44,
  "safe": i45,
  "search": i46,
  "send": i47,
  "shop": i48,
  "stack": i49,
  "stars": i50,
  "tag": i51,
  "tags": i52,
  "telephone": i53,
  "truck": i54,
  "upc-scan": i55,
  "wallet2": i56,
})

export const DEFAULT_SAVED_VIEW_ICON = 'funnel'

export const SAVED_VIEW_ICONS = [
  { id: "archive", name: msg("manage.views.archive"), icon: "archive" },
  { id: "bank", name: msg("manage.views.bank"), icon: "bank" },
  { id: "basket", name: msg("manage.views.basket"), icon: "basket" },
  { id: "bell", name: msg("manage.views.bell"), icon: "bell" },
  { id: "bookmark", name: msg("manage.views.bookmark"), icon: "bookmark" },
  { id: "boxes", name: msg("manage.views.boxes"), icon: "boxes" },
  { id: "briefcase", name: msg("manage.views.briefcase"), icon: "briefcase" },
  { id: "building", name: msg("manage.views.building"), icon: "building" },
  { id: "calculator", name: msg("manage.views.calculator"), icon: "calculator" },
  { id: "calendar", name: msg("manage.views.calendar"), icon: "calendar" },
  { id: "camera", name: msg("manage.views.camera"), icon: "camera" },
  { id: "card-checklist", name: msg("manage.views.checklist"), icon: "card-checklist" },
  { id: "cash", name: msg("manage.views.cash"), icon: "cash" },
  { id: "chat-left-text", name: msg("manage.views.chat"), icon: "chat-left-text" },
  { id: "check-circle", name: msg("manage.views.check"), icon: "check-circle" },
  { id: "clipboard", name: msg("manage.views.clipboard"), icon: "clipboard" },
  { id: "clock-history", name: msg("manage.views.clock"), icon: "clock-history" },
  { id: "credit-card", name: msg("manage.views.credit_card"), icon: "credit-card" },
  { id: "download", name: msg("common.download"), icon: "download" },
  { id: "envelope", name: msg("manage.views.envelope"), icon: "envelope" },
  { id: "exclamation-triangle", name: msg("manage.views.warning"), icon: "exclamation-triangle" },
  { id: "file-earmark", name: msg("manage.views.file"), icon: "file-earmark" },
  { id: "file-earmark-check", name: msg("manage.views.checked_file"), icon: "file-earmark-check" },
  { id: "file-earmark-lock", name: msg("manage.views.locked_file"), icon: "file-earmark-lock" },
  { id: "file-earmark-medical", name: msg("manage.views.medical_file"), icon: "file-earmark-medical" },
  { id: "file-earmark-person", name: msg("manage.views.person_file"), icon: "file-earmark-person" },
  { id: "file-earmark-spreadsheet", name: msg("manage.views.spreadsheet"), icon: "file-earmark-spreadsheet" },
  { id: "file-text", name: msg("manage.views.text_file"), icon: "file-text" },
  { id: "files", name: msg("manage.views.files"), icon: "files" },
  { id: "folder", name: msg("common.folder"), icon: "folder" },
  { id: "funnel", name: msg("common.filter"), icon: "funnel" },
  { id: "gear", name: msg("manage.views.gear"), icon: "gear" },
  { id: "globe2", name: msg("manage.views.globe"), icon: "globe2" },
  { id: "hash", name: msg("manage.views.hash"), icon: "hash" },
  { id: "heart", name: msg("manage.views.heart"), icon: "heart" },
  { id: "house", name: msg("manage.views.house"), icon: "house" },
  { id: "inbox", name: msg("common.inbox"), icon: "inbox" },
  { id: "journals", name: msg("manage.views.journals"), icon: "journals" },
  { id: "list-task", name: msg("manage.views.task_list"), icon: "list-task" },
  { id: "newspaper", name: msg("manage.views.newspaper"), icon: "newspaper" },
  { id: "paperclip", name: msg("manage.views.attachment"), icon: "paperclip" },
  { id: "people", name: msg("manage.views.people"), icon: "people" },
  { id: "person", name: msg("manage.views.person"), icon: "person" },
  { id: "printer", name: msg("manage.views.printer"), icon: "printer" },
  { id: "receipt", name: msg("manage.views.receipt"), icon: "receipt" },
  { id: "safe", name: msg("manage.views.safe"), icon: "safe" },
  { id: "search", name: msg("common.search"), icon: "search" },
  { id: "send", name: msg("common.send"), icon: "send" },
  { id: "shop", name: msg("manage.views.shop"), icon: "shop" },
  { id: "stack", name: msg("manage.views.stack"), icon: "stack" },
  { id: "stars", name: msg("manage.views.stars"), icon: "stars" },
  { id: "tag", name: msg("manage.views.tag"), icon: "tag" },
  { id: "tags", name: msg("common.tags"), icon: "tags" },
  { id: "telephone", name: msg("manage.views.telephone"), icon: "telephone" },
  { id: "truck", name: msg("manage.views.truck"), icon: "truck" },
  { id: "upc-scan", name: msg("manage.views.barcode"), icon: "upc-scan" },
  { id: "wallet2", name: msg("manage.views.wallet"), icon: "wallet2" },
]
