import { describe, expect, it } from 'vitest'
import { daysRemaining } from './trash/trash'
import { getLogLevel, hasChanges, parseLogsWithLevel } from './logs/logs'
import { categoryOptions, categorySections, configToForm, formToSave, isConfigDirty } from './config/config'
import { ConfigCategory } from './config/config-options'
import { entriesFromValue, valueFromEntries } from '../common/input/entries/entries'
import { valuesForSelected } from '../common/input/custom-fields-values/custom-fields-values'
import { passwordIsSet, userFormValues } from '../common/edit-dialog/user-edit-dialog/user-edit'
import { copyRule, showActionParamField, showCorrespondentField } from '../manage/mail/mail-options'

describe('Administration (Logik)', () => {
  it('Papierkorb: verbleibende Tage = Verzögerung minus angefangene Tage', () => {
    const now = new Date('2026-11-09T12:00:00Z')
    expect(daysRemaining({ deleted_at: '2026-11-08T13:00:00Z' }, 30, now)).toBe(29)
    expect(daysRemaining({ deleted_at: '2026-11-01T12:00:00Z' }, 30, now)).toBe(22)
    // expired (A-131): never negative
    expect(daysRemaining({ deleted_at: '2026-01-01T12:00:00Z' }, 30, now)).toBe(0)
    expect(daysRemaining({ deleted_at: 'kaputt' }, 30, now)).toBe(0)
  })
  it('Protokolle: Stufen und Änderungserkennung', () => {
    expect(['[DEBUG]', '[WARNING]', '[ERROR]', '[CRITICAL]', '[INFO]'].map(getLogLevel)).toEqual([10, 30, 40, 50, 20])
    const a = parseLogsWithLevel(['x [ERROR] y'])
    expect(hasChanges(a, a)).toBe(false)
    expect(hasChanges(parseLogsWithLevel(['z']), a)).toBe(true)
  })
  it('Konfiguration: Abschnitte, Formular, Änderung, Logo nicht als Text senden', () => {
    expect(categorySections(ConfigCategory.OCR)).toContain(null)
    expect(categoryOptions(ConfigCategory.General).length).toBeGreaterThan(0)
    const f = configToForm({ id: 1, app_title: null, app_logo: '/logo/x.png' })
    expect(isConfigDirty({ ...f, app_title: 'X' }, f)).toBe(true)
    expect(isConfigDirty({ ...f }, f)).toBe(false)
    expect(formToSave(f)).not.toHaveProperty('app_logo')
  })
  it('Eingaben: Einträge ohne leere Schlüssel, Feldwerte nur gewählter Felder', () => {
    expect(valueFromEntries([['a', '1'], ['', '2']])).toEqual({ a: '1' })
    expect(entriesFromValue(null)).toEqual([])
    expect(valuesForSelected([1, 2], { 1: 'x', 3: 'y' })).toEqual({ 1: 'x', 2: null })
  })
  it('Benutzer: deaktivierte Felder fehlen, Passwort aus Sternchen zählt nicht', () => {
    expect(userFormValues({ is_superuser: true, user_permissions: ['a'] }, true)).toEqual({ is_superuser: true })
    expect(userFormValues({ is_superuser: false, user_permissions: [] }, false)).toEqual({ user_permissions: [] })
    expect(passwordIsSet('********')).toBe(false)
    expect(passwordIsSet('neu')).toBe(true)
  })
  it('E-Mail-Regeln: Kopie, Zusatzfelder je Aktion/Korrespondent', () => {
    expect(copyRule({ id: 3, name: 'R' })).toEqual({ id: null, name: 'R (copy)' })
    expect([1, 2, 3, 4, 5].map(showActionParamField)).toEqual([false, true, false, false, true])
    expect(showCorrespondentField(4)).toBe(true)
  })
})
