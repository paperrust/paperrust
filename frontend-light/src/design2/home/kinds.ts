// Label of the date kinds (contract: created, due, cancel_by, valid_from, valid_until, warranty_until)
import { t, msg } from '../../lib/i18n'

const KIND: Record<string, string> = {
  created: msg('m.timeline.document_date'),
  due: msg('m.timeline.due'),
  cancel_by: msg('m.timeline.cancel'),
  valid_from: msg('m.timeline.valid'),
  valid_until: msg('m.timeline.valid_until'),
  warranty_until: msg('m.timeline.warranty_until'),
}
export const kindLabel = (k: string) => (KIND[k] ? t(KIND[k]) : k)
