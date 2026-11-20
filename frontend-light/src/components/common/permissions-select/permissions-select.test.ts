import { describe, expect, it } from 'vitest'
import { allowedTypes, checkedState, isInherited, selectedPermissions, SORTED_ACTIONS, stripAppLabel, typeHasAllActions } from './permissions-select'

describe('PermissionsSelectComponent', () => {
  it('Typen ohne Verlauf bei abgeschaltetem Audit-Log, Aktionen in Reihenfolge Add/Change/Delete/View', () => {
    expect(allowedTypes(false)).not.toContain('History')
    expect(allowedTypes(true)).toContain('History')
    expect(SORTED_ACTIONS).toEqual(['Add', 'Change', 'Delete', 'View'])
  })
  it('geerbte Rechte (App-Präfix entfernt) sind angehakt, werden aber nicht gemeldet', () => {
    const inherited = stripAppLabel(['documents.view_document', 'documents.add_document', 'documents.change_document', 'documents.delete_document'])
    expect(inherited).toContain('view_document')
    expect(isInherited(inherited, 'Document')).toBe(true)
    const grid = checkedState(['Document', 'Tag'], ['view_tag'], inherited)
    expect(grid.Document.View && grid.Tag.View && !grid.Tag.Add).toBe(true)
    expect(typeHasAllActions(grid.Document, 'Document')).toBe(true)
    grid.Tag.Add = true
    expect(selectedPermissions(grid, inherited)).toEqual(['add_tag', 'view_tag'])
  })
  it('Systemstatus/Statistik kennen nur „ansehen“', () => {
    const grid = checkedState(['SystemMonitoring'], ['add_system_monitoring', 'view_system_monitoring'], [])
    expect(grid.SystemMonitoring).toMatchObject({ Add: false, View: true })
    expect(typeHasAllActions(grid.SystemMonitoring, 'SystemMonitoring')).toBe(true)
  })
})
