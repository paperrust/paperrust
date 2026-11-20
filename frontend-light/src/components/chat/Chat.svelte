<!-- Port of chat/chat (ChatComponent): navbar dropdown "Questions about documents", POST /api/documents/chat/
     (streamed, typewriter effect 10 ms/character), source links. Loaded lazily by AppFrame only with ai_enabled.
     Host <pngx-chat> in the markup of AppFrame. Styles from chat.component.scss below (encapsulated under pngx-chat). -->
<script lang="ts">
  import { t } from '../../lib/i18n'
  import { icon } from '../common/bootstrap-icon/icon'
  import { registerIcons, ricon } from '../../lib/route-icons'
  import iChat from 'bootstrap-icons/icons/chat-square-dots.svg?raw'
  import iReturn from 'bootstrap-icons/icons/arrow-return-left.svg?raw'
  // only needed in the (lazily loaded) chat: not in the icon list of the first call
  registerIcons({ 'chat-square-dots': iChat, 'arrow-return-left': iReturn })
  import { Dropdown } from '../../lib/dropdown.svelte'
  import { router } from '../../router.svelte'
  import { streamChat, parseChatResponse, type ChatMessage } from './chat-service'
  import { renderMarkdown } from './markdown'

  const SLOW_RESPONSE_DELAY_MS = 8000

  let messages = $state<ChatMessage[]>([])
  let loading = $state(false)
  let input = $state('')
  let slowResponse = $state(false)
  let scrollAnchor: HTMLDivElement | undefined = $state()
  let chatInput: HTMLInputElement | undefined = $state()

  const documentId = $derived.by(() => {
    const m = router.path.match(/^documents\/(\d+)/)
    return m ? +m[1] : undefined
  })
  const placeholder = $derived(documentId ? t('chat.placeholder_document') : t('chat.placeholder_any'))

  const dd = new Dropdown({
    placement: 'bottom-end',
    pngxPopper: true,
    onOpenChange: (open) => {
      if (open) setTimeout(() => chatInput?.focus(), 10)
    },
  })

  let typewriterBuffer: string[] = []
  let typewriterActive = false
  let typewriterTimeout: ReturnType<typeof setTimeout> | undefined
  let abort: AbortController | undefined
  let streamingMessage: ChatMessage | undefined
  let streamFinished = false
  let slowResponseTimeout: ReturnType<typeof setTimeout> | undefined

  function sendMessage() {
    if (!input.trim()) return
    const prompt = input
    messages.push({ role: 'user', content: prompt })
    scrollToBottom()
    messages.push({ role: 'assistant', content: '', isStreaming: true })
    const assistantMessage = messages[messages.length - 1]
    loading = true
    streamingMessage = assistantMessage
    streamFinished = false
    slowResponseTimeout = setTimeout(() => (slowResponse = true), SLOW_RESPONSE_DELAY_MS)

    let lastVisibleContent = ''
    const controller = new AbortController()
    abort = controller
    streamChat(
      documentId,
      prompt,
      (chunk) => {
        if (controller.signal.aborted) return
        clearSlowResponse()
        const next = parseChatResponse(chunk)
        if (next.content.length < lastVisibleContent.length) {
          resetTypewriter(assistantMessage, next.content)
          lastVisibleContent = next.content
        } else {
          const visibleDelta = next.content.substring(lastVisibleContent.length)
          lastVisibleContent = next.content
          enqueueTypewriter(visibleDelta, assistantMessage)
        }
        assistantMessage.references = next.references
      },
      controller.signal,
    )
      .then(() => {
        if (controller.signal.aborted) return
        // let the typewriter catch up before hiding the cursor
        streamFinished = true
        if (!typewriterActive) finishStreaming(assistantMessage)
      })
      .catch(() => {
        if (controller.signal.aborted) return
        flushTypewriter(assistantMessage)
        assistantMessage.content += '\n\n⚠️ Error receiving response.'
        finishStreaming(assistantMessage)
      })

    input = ''
  }

  function stopStreaming() {
    const message = streamingMessage
    if (!message?.isStreaming) return
    abort?.abort()
    flushTypewriter(message)
    if (!message.content) messages = messages.filter((m) => m !== message)
    finishStreaming(message)
  }

  function clearChat() {
    stopStreaming()
    messages = []
    setTimeout(() => chatInput?.focus())
  }

  function finishStreaming(message: ChatMessage) {
    clearSlowResponse()
    streamFinished = false
    message.isStreaming = false
    loading = false
    scrollToBottom()
  }

  function clearSlowResponse() {
    clearTimeout(slowResponseTimeout)
    slowResponse = false
  }

  function flushTypewriter(message: ChatMessage) {
    message.content += typewriterBuffer.join('')
    cancelTypewriter()
  }

  function cancelTypewriter() {
    clearTimeout(typewriterTimeout)
    typewriterBuffer = []
    typewriterActive = false
  }

  function resetTypewriter(message: ChatMessage, content: string) {
    cancelTypewriter()
    message.content = content
    scrollToBottom()
  }

  function enqueueTypewriter(chunk: string, message: ChatMessage) {
    if (!chunk) return
    typewriterBuffer.push(...chunk.split(''))
    if (!typewriterActive) {
      typewriterActive = true
      playTypewriter(message)
    }
  }

  function playTypewriter(message: ChatMessage) {
    if (typewriterBuffer.length === 0) {
      typewriterActive = false
      if (streamFinished) finishStreaming(message)
      return
    }
    message.content += typewriterBuffer.shift()
    scrollToBottom()
    // 10ms per character
    typewriterTimeout = setTimeout(() => playTypewriter(message), 10)
  }

  function scrollToBottom() {
    setTimeout(() => scrollAnchor?.scrollIntoView({ behavior: 'smooth' }), 50)
  }

  function searchInputKeyDown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !(event.isComposing || event.keyCode === 229)) {
      event.preventDefault()
      sendMessage()
    }
  }
</script>

<li class="nav-item {dd.placementClass}" class:show={dd.open} use:dd.hostEl>
  <button class="btn navbar-action border-0 dropdown-toggle" id="chatDropdown" aria-label={t('manage.views.chat')} aria-expanded={dd.open} use:dd.anchorEl onclick={dd.toggle}>
    <pngx-bs-icon width="1.3em" height="1.3em" name="chatSquareDots">{@html ricon('chat-square-dots', '1.3em', '1.3em')}</pngx-bs-icon>
  </button>
  {#if dd.open}
    <div class="dropdown-menu dropdown-menu-end shadow p-0 overflow-hidden show" aria-labelledby="chatDropdown" use:dd.menuEl>
      <div class="d-flex align-items-center border-bottom px-3 py-2">
        <pngx-bs-icon width="1em" height="1em" name="stars" class="text-primary me-2">{@html icon('stars', '1em', '1em')}</pngx-bs-icon>
        <span class="fw-semibold">
          {#if documentId}
            {t('chat.title_document')}
          {:else}
            {t('chat.title_any')}
          {/if}
        </span>
        {#if messages.length}
          <button class="btn btn-sm btn-link text-muted ms-auto p-0" type="button" onclick={clearChat} title={t('chat.clear')}>
            <pngx-bs-icon width="1em" height="1em" name="arrow-counterclockwise">{@html icon('arrow-counterclockwise', '1em', '1em')}</pngx-bs-icon>
          </button>
        {/if}
      </div>

      <div class="chat-messages px-2">
        {#if !messages.length}
          <p class="text-muted text-center py-4 px-3 mb-0">{t('chat.empty_hint')}</p>
        {/if}
        {#each messages as message, idx (message)}
          <div class="message d-flex flex-row" class:justify-content-end={message.role === 'user'} class:mt-3={message.role === 'user' && idx !== 0}>
            <div class="p-2 m-2" class:bg-body-secondary={message.role === 'user'} class:rounded-3={message.role === 'user'} class:flex-grow-1={message.role === 'assistant'}>
              {#if message.isStreaming && !message.content}
                <div class="d-flex align-items-center text-muted">
                  <div class="spinner-border spinner-border-sm me-2" role="status"></div>
                  {#if slowResponse}
                    {t('chat.still_thinking')}
                  {:else}
                    {t('chat.thinking')}
                  {/if}
                </div>
              {:else if message.role === 'assistant'}
                <div class="chat-markdown text-break text-wrap" class:streaming={message.isStreaming}>{@html renderMarkdown(message.content)}</div>
              {:else}
                <span class="text-break">{message.content}</span>
              {/if}
              {#if message.role === 'assistant' && !message.isStreaming && message.references?.length}
                <div class="chat-references list-group mt-3">
                  <div class="list-group-item text-muted small py-1">{t('m.assistant.sources')}</div>
                  {#each message.references as reference (reference.id)}
                    <a class="list-group-item list-group-item-action d-flex align-items-center text-primary" href={router.href(`documents/${reference.id}`)} title={reference.title} onclick={() => dd.close()}>
                      <pngx-bs-icon width="0.9em" height="0.9em" name="file-text" class="me-2 flex-shrink-0">{@html icon('file-text', '0.9em', '0.9em')}</pngx-bs-icon><span class="text-truncate">{reference.title}</span>
                    </a>
                  {/each}
                </div>
              {/if}
            </div>
          </div>
        {/each}
        <div bind:this={scrollAnchor}></div>
      </div>

      <form class="chat-input border-top p-2" onsubmit={(e) => e.preventDefault()}>
        <label class="visually-hidden" for="chatInput">{t('ui.message')}</label>
        <div class="input-group">
          <input bind:this={chatInput} id="chatInput" class="form-control" name="chatInput" type="text" {placeholder} disabled={loading} bind:value={input} onkeydown={searchInputKeyDown} />
          {#if loading}
            <button class="btn btn-outline-secondary" type="button" onclick={stopStreaming} title={t('m.assistant.stop')} aria-label={t('m.assistant.stop')}>
              <pngx-bs-icon width="1em" height="1em" name="x-lg">{@html icon('x-lg', '1em', '1em')}</pngx-bs-icon>
            </button>
          {:else}
            <button class="btn btn-primary" type="button" onclick={sendMessage} title={t('common.send')} aria-label={t('common.send')}>
              <pngx-bs-icon width="1em" height="1em" name="arrow-return-left">{@html ricon('arrow-return-left', '1em', '1em')}</pngx-bs-icon>
            </button>
          {/if}
        </div>
      </form>
    </div>
  {/if}
</li>

<style>
  /* chat.component.scss (Angular encapsulates per component; here under the host pngx-chat) */
  :global(pngx-chat .dropdown-menu) {
    width: min(36rem, calc(100vw - 20px));
  }
  :global(pngx-chat .chat-messages) {
    max-height: min(60dvh, 36rem);
    overflow-y: auto;
    white-space: pre-wrap;
  }
  :global(pngx-chat .dropdown-toggle::after) {
    display: none;
  }
  @media screen and (max-width: 575.98px) {
    :global(pngx-chat .chat-messages) {
      max-height: calc(100dvh - 12rem);
    }
  }
  :global(pngx-chat .chat-markdown.streaming > :last-child:not(ul, ol, table)::after),
  :global(pngx-chat .chat-markdown.streaming > :is(ul, ol):last-child > li:last-child::after) {
    content: '';
    display: inline-block;
    width: 2px;
    height: 1.05em;
    margin-left: 2px;
    background-color: currentColor;
    vertical-align: -0.15em;
    animation: pngx-chat-blink 1s step-end infinite;
  }
  @keyframes -global-pngx-chat-blink {
    50% {
      opacity: 0;
    }
  }
  :global(pngx-chat .chat-markdown :is(h1, h2, h3, h4, h5, h6)) {
    font-size: 1em;
    font-weight: bold;
  }
  :global(pngx-chat .chat-markdown :is(th, td)) {
    padding: 0.15rem 0.4rem;
    border: 1px solid var(--bs-border-color);
  }
  :global(pngx-chat .chat-markdown > :last-child) {
    margin-bottom: 0;
  }
</style>
