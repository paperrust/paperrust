import { describe, expect, it } from 'vitest'
import { CHAT_METADATA_DELIMITER, parseChatResponse, streamChat } from './chat-service'
import { renderInline, renderMarkdown } from './markdown'

describe('parseChatResponse', () => {
  it('splits content and references', () => {
    expect(parseChatResponse('hello')).toEqual({ content: 'hello' })
    expect(parseChatResponse(`hi${CHAT_METADATA_DELIMITER}{"references":[{"id":1,"title":"A"}]}`)).toEqual({ content: 'hi', references: [{ id: 1, title: 'A' }] })
    expect(parseChatResponse(`hi${CHAT_METADATA_DELIMITER}{"refer`)).toEqual({ content: 'hi' })
  })
})

describe('streamChat', () => {
  it('reports the accumulated text per chunk', async () => {
    const enc = new TextEncoder()
    const body = new ReadableStream({
      start(c) {
        c.enqueue(enc.encode('Hel'))
        c.enqueue(enc.encode('lo'))
        c.close()
      },
    })
    let req: any
    const parts: string[] = []
    const fakeFetch = async (url: string, init: any) => {
      req = { url, init }
      return new Response(body, { status: 200 })
    }
    await streamChat(5, 'q?', (p) => parts.push(p), undefined, fakeFetch as any)
    expect(parts).toEqual(['Hel', 'Hello'])
    expect(req.url).toBe('/api/documents/chat/')
    expect(JSON.parse(req.init.body)).toEqual({ document_id: 5, q: 'q?' })
  })
  it('rejects on HTTP errors', async () => {
    const fakeFetch = async () => new Response('x', { status: 500 })
    await expect(streamChat(undefined, 'q', () => {}, undefined, fakeFetch as any)).rejects.toThrow()
  })
})

describe('markdown', () => {
  it('escapes raw HTML', () => {
    expect(renderMarkdown('<script>alert(1)</script>')).toBe('<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>')
    expect(renderInline('<img src=x onerror=alert(1)>')).toBe('&lt;img src=x onerror=alert(1)&gt;')
  })
  it('only allows http/https/mailto links', () => {
    expect(renderInline('[x](javascript:alert)')).toBe('x')
    expect(renderInline('[doc](https://example.com/a)')).toBe(
      '<a target="_blank" rel="noopener noreferrer nofollow" href="https://example.com/a">doc</a> <span class="text-muted small">(example.com)</span>',
    )
    expect(renderInline('![alt](https://example.com/i.png)')).toBe('![alt](https://example.com/i.png)')
  })
  it('inline formatting', () => {
    expect(renderInline('**b** *i* `c<d>` ~~s~~')).toBe('<strong>b</strong> <em>i</em> <code>c&lt;d&gt;</code> <del>s</del>')
  })
  it('blocks: paragraphs with breaks, lists, headings, code, tables', () => {
    expect(renderMarkdown('a\nb\n\nc')).toBe('<p>a<br>b</p>\n<p>c</p>')
    expect(renderMarkdown('- x\n- y')).toBe('<ul><li>x</li><li>y</li></ul>')
    expect(renderMarkdown('2. x\n3. y')).toBe('<ol start="2"><li>x</li><li>y</li></ol>')
    expect(renderMarkdown('## T')).toBe('<h2>T</h2>')
    expect(renderMarkdown('```\n<b>\n```')).toBe('<pre><code>&lt;b&gt;\n</code></pre>')
    expect(renderMarkdown('| a | b |\n|---|---|\n| 1 | 2 |')).toBe(
      '<table><thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td><td>2</td></tr></tbody></table>',
    )
    expect(renderMarkdown('text\n- item')).toBe('<p>text</p>\n<ul><li>item</li></ul>')
  })
})
