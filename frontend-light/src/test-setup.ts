// Vitest: load the English catalog so that the translation function returns English texts in tests (key → text).
import { setDictionary } from './lib/i18n'
import en from '../i18n/en-US.json'

setDictionary('en-US', en as Record<string, string>)
