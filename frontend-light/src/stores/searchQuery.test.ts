import { describe, expect, it } from 'vitest'
import { getSearchQuery, searchQueryFromRules, setSearchQuery, withSearchHash } from './searchQuery'

describe('searchQuery', () => {
  it('takes the term from text filter rules', () => {
    expect(searchQueryFromRules([{ rule_type: 6, value: '1' }, { rule_type: 20, value: 'foo bar' }])).toBe('foo bar')
    expect(searchQueryFromRules([{ rule_type: 49, value: 'x' }])).toBe('x')
    expect(searchQueryFromRules([{ rule_type: 6, value: '1' }])).toBe('')
  })
  it('adds #search like getPreviewUrl', () => {
    setSearchQuery('  ')
    expect(getSearchQuery()).toBe('')
    expect(withSearchHash('http://h/api/documents/1/preview/')).toBe('http://h/api/documents/1/preview/')
    setSearchQuery(' foo bar ')
    expect(withSearchHash('http://h/api/documents/1/preview/?version=2')).toBe('http://h/api/documents/1/preview/?version=2#search=%22foo%20bar%22')
    expect(withSearchHash(undefined)).toBeUndefined()
    setSearchQuery('')
  })
})
