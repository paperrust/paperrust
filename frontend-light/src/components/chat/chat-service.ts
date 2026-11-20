// Port of services/chat.service.ts: POST /api/documents/chat/ with a streamed text response. Angular reports
// `partialText` (everything received so far) per progress event; streamChat() does the same via fetch + reader.
import { getCookie } from '../../api/v1'

export interface ChatReference {
  id: number
  title: string
}

export interface ChatMessage {
  role: 'user' | 'assistant'
  content: string
  isStreaming?: boolean
  references?: ChatReference[]
}

export interface ParsedChatResponse {
  content: string
  references?: ChatReference[]
}

export const CHAT_METADATA_DELIMITER = '\n\n__PAPERLESS_CHAT_METADATA__'

/** parseChatResponse: text before the delimiter, JSON metadata ({ references }) after it */
export function parseChatResponse(response: string): ParsedChatResponse {
  const delimiterIndex = response.indexOf(CHAT_METADATA_DELIMITER)
  if (delimiterIndex === -1) return { content: response }
  const metadataString = response.slice(delimiterIndex + CHAT_METADATA_DELIMITER.length)
  try {
    const metadata = JSON.parse(metadataString) as { references?: ChatReference[] }
    return { content: response.slice(0, delimiterIndex), references: metadata.references ?? [] }
  } catch {
    return { content: response.slice(0, delimiterIndex) }
  }
}

/**
 * ChatService.streamChat(documentId, prompt): calls `onPartial(textSoFar)` for each received chunk; resolves on
 * completion, rejects on HTTP/network errors. Abort via `signal` resolves silently.
 */
export async function streamChat(
  documentId: number | undefined,
  prompt: string,
  onPartial: (partialText: string) => void,
  signal?: AbortSignal,
  fetchFn: typeof fetch = (i, init) => fetch(i, init),
): Promise<void> {
  const headers: Record<string, string> = { Accept: 'application/json, text/plain, */*', 'Content-Type': 'application/json' }
  const csrf = getCookie('csrftoken')
  if (csrf) headers['X-CSRFToken'] = csrf
  let res: Response
  try {
    res = await fetchFn('/api/documents/chat/', {
      method: 'POST',
      headers,
      credentials: 'same-origin',
      body: JSON.stringify({ document_id: documentId, q: prompt }),
      signal,
    })
  } catch (e) {
    if (signal?.aborted) return
    throw e
  }
  if (!res.ok) throw new Error(`HTTP ${res.status}`)
  const decoder = new TextDecoder()
  let text = ''
  if (!res.body) {
    text = await res.text()
    if (text) onPartial(text)
    return
  }
  const reader = res.body.getReader()
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      text += decoder.decode(value, { stream: true })
      if (text) onPartial(text)
    }
    const rest = decoder.decode()
    if (rest) {
      text += rest
      onPartial(text)
    }
  } catch (e) {
    if (signal?.aborted) return
    throw e
  }
}
