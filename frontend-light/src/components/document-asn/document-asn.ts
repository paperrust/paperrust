// Logic of DocumentAsnComponent (document-asn/document-asn.component.ts).
import { api } from '../../api/client'
import { queryString, type Paged } from '../../api/v1'

/** DocumentService.listAllFilteredIds([{ rule_type: FILTER_ASN, value: asn }]) – same parameters and order */
export async function findDocumentIdsByAsn(asn: string): Promise<number[]> {
  const res = await api.get<Paged<{ id: number }>>(`/api/documents/${queryString({ page: 1, page_size: 100000, fields: 'id', archive_serial_number: asn })}`)
  return (res?.results ?? []).map((d) => d.id)
}

/** exactly one hit → documents/<id>, otherwise 404 */
export function asnTarget(ids: number[]): string {
  return ids.length == 1 ? `documents/${ids[0]}` : '404'
}
