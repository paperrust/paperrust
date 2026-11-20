<!-- Assistant (concept E): questions to one's own archive, answer streamed, sources as chips ([n] in the text jumps to the
     source), actions (open documents, create reminder). API: POST /rs/api/v2/ask (SSE). -->
<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { t, msg } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { toastService } from '../../stores/toasts.svelte'
  import { ui } from '../stores/ui.svelte'
  import { ask, conversation as loadConversation, conversations as listConversations, mockEnabled, runRequest, Unavailable, type AskAction, type Citation, type Conversation } from '../api/assist'
  import Icon from '../components/Icon.svelte'

  interface Msg {
    role: 'user' | 'assistant'
    text: string
    citations: Citation[]
    actions: AskAction[]
    notes: string[]
    error?: string | null
    streaming?: boolean
    /** AI off / no LLM: only weighted hits (contract: answer null, notes) */
    sourcesOnly?: boolean
  }
  let msgs = $state<Msg[]>([])
  let q = $state(new URLSearchParams(router.query).get('q') ?? '')
  let busy = $state(false)
  let conversation = $state<string | null>(null)
  let convs = $state<Conversation[]>([])
  let unavailable = $state(false)
  let scroller: HTMLDivElement
  let input: HTMLTextAreaElement
  let ctrl: AbortController | null = null

  const aiOn = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  const SUGGEST = [msg('m.assistant.invoices_due_month'), msg('m.assistant.insurance_contract_end'), msg('m.assistant.summarize_last_letter_bank')]

  onMount(() => {
    listConversations()
      .then((c) => (convs = c))
      .catch((e) => e instanceof Unavailable && (unavailable = !mockEnabled()))
    if (q) send()
    else input?.focus()
    return () => ctrl?.abort()
  })

  async function scrollDown() {
    await tick()
    scroller?.scrollTo({ top: scroller.scrollHeight })
  }

  async function send(text = q) {
    const question = text.trim()
    if (!question || busy) return
    q = ''
    msgs = [...msgs, { role: 'user', text: question, citations: [], actions: [], notes: [] }, { role: 'assistant', text: '', citations: [], actions: [], notes: [], streaming: true }]
    const i = msgs.length - 1
    const upd = (f: (m: Msg) => void) => {
      const m = { ...msgs[i] }
      f(m)
      msgs[i] = m
    }
    busy = true
    ctrl = new AbortController()
    scrollDown()
    try {
      await ask(
        question,
        conversation,
        (e) => {
          if (e.type === 'meta') {
            if (e.conversation) conversation = e.conversation
            if (e.notes?.length) upd((m) => (m.notes = e.notes!))
          } else if (e.type === 'delta') upd((m) => (m.text += e.text))
          else if (e.type === 'citations') upd((m) => (m.citations = e.citations))
          else if (e.type === 'actions') upd((m) => (m.actions = e.actions))
          else if (e.type === 'error') upd((m) => (m.error = e.message))
          else if (e.type === 'done')
            upd((m) => {
              if (!m.text && e.answer) m.text = e.answer
              if (e.answerError) m.error = e.answerError
              if (e.notes?.length) m.notes = e.notes
              if (!m.text && !e.answer && !e.answerError && !m.error) m.sourcesOnly = true
            })
          // finished answer: jump to its start (hints/sources visible at the top), otherwise follow along
          if (e.type === 'done') tick().then(() => scroller?.querySelectorAll('.msg.assistant')[i >> 1]?.scrollIntoView({ block: 'start' }))
          else scrollDown()
        },
        ctrl.signal,
      )
    } catch (e: any) {
      if (e instanceof Unavailable) {
        unavailable = true
        upd((m) => (m.error = t('m.assistant.assistant_not_available_server')))
      } else if (e?.name !== 'AbortError') upd((m) => (m.error = e?.message ?? String(e)))
    } finally {
      upd((m) => (m.streaming = false))
      busy = false
      ctrl = null
      refreshConvs()
      input?.focus()
    }
  }

  function stop() {
    ctrl?.abort()
  }

  /** Split answer text into text and [n] references */
  function parts(text: string): (string | number)[] {
    return text.split(/\[(\d+)\]/).map((p, i) => (i % 2 ? +p : p))
  }
  function cite(m: Msg, n: number) {
    return m.citations.find((c) => c.n === n)
  }
  const openDoc = (id: number) => router.navigate('documents/' + id)
  async function act(a: AskAction) {
    if (a.type === 'open_documents' && a.legacyIds?.length) return openDoc(a.legacyIds[0])
    if (a.request) {
      try {
        await runRequest(a.request)
        toastService.showSuccess(a.type === 'create_reminder' ? t('m.assistant.reminder_added_timeline') : t('common.done'))
      } catch (e) {
        toastService.showError(t('m.assistant.action_failed'), e)
      }
    }
  }
  async function openConversation(uuid: string) {
    ctrl?.abort()
    try {
      const ms = await loadConversation(uuid)
      conversation = uuid
      msgs = ms.map((m) => ({
        role: m.role,
        text: m.content,
        citations: m.citations,
        actions: m.actions,
        notes: m.notes,
        error: m.answerError,
        sourcesOnly: m.role === 'assistant' && !m.content && !m.answerError,
      }))
      scrollDown()
    } catch (e) {
      toastService.showError(t('m.assistant.could_not_load_conversation'), e)
    }
  }
  const refreshConvs = () => listConversations().then((c) => (convs = c)).catch(() => {})
  function newChat() {
    ctrl?.abort()
    msgs = []
    conversation = null
    input?.focus()
  }
  function onkey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault()
      send()
    }
  }
</script>

<div class="as" class:mobile={ui.mobile}>
  {#if !ui.mobile}
    <aside class="side" aria-label={t('m.assistant.conversations')}>
      <div class="d2-pane-head"><h2>{t('m.assistant.conversations')}</h2></div>
      <div class="d2-scroll list">
        <button class="d2-btn primary new" onclick={newChat}><Icon name="plus-lg" />{t('m.assistant.new_conversation')}</button>
        {#each convs as c (c.uuid)}
          <button class="conv" class:on={c.uuid === conversation} title={c.title} onclick={() => openConversation(c.uuid)}><Icon name="chat-left-text" /><span>{c.title}</span></button>
        {:else}
          <p class="d2-muted small">{t('m.assistant.earlier_conversations_appear_here')}</p>
        {/each}
      </div>
    </aside>
  {/if}
  <section class="chat" aria-label={t('common.assistant')}>
    <header class="d2-pane-head">
      <h1><Icon name="stars" /> {t('common.assistant')}</h1>
      {#if mockEnabled()}<span class="d2-chip demo">{t('common.demo_data')}</span>{/if}
      <span class="grow"></span>
      {#if ui.mobile}<button class="d2-btn ghost sm" onclick={newChat}><Icon name="plus-lg" />{t('m.assistant.new')}</button>{/if}
    </header>
    <div class="msgs d2-scroll" bind:this={scroller} aria-live="polite" aria-busy={busy} id="d2-chat-log">
      {#if msgs.length === 0}
        <div class="intro">
          <Icon name="stars" size="2.5rem" />
          <h2>{t('m.assistant.ask_archive')}</h2>
          <p class="d2-muted">{t('m.assistant.answers_come_only_documents')}</p>
          {#if !aiOn && !unavailable}<p class="hint" role="note" id="d2-ai-off-intro"><Icon name="info-circle" /> {t('m.assistant.ai_switched_off_server')}</p>{/if}
          {#if unavailable}<p class="warn" role="note">{t('m.assistant.assistant_not_available_server')}</p>{/if}
          <div class="sugg">
            {#each SUGGEST as s (s)}<button class="d2-btn" onclick={() => send(t(s))}>{t(s)}</button>{/each}
          </div>
        </div>
      {/if}
      {#each msgs as m, i (i)}
        <div class="msg {m.role}">
          {#if m.role === 'user'}
            <p class="bubble">{m.text}</p>
          {:else}
            <div class="answer">
              {#if m.text}
                <p class="txt">
                  {#each parts(m.text) as p, j (j)}
                    {#if typeof p === 'number'}
                      {@const c = cite(m, p)}
                      {#if c}<button class="ref" onclick={() => openDoc(c.document.id)} title={c.document.title} aria-label={t('m.assistant.source', { PH: p, PH_1: c.document.title })}>{p}</button>{:else}[{p}]{/if}
                    {:else}{p}{/if}
                  {/each}
                  {#if m.streaming}<span class="caret" aria-hidden="true"></span>{/if}
                </p>
              {:else if m.streaming}
                <p class="d2-muted">{t('m.assistant.searching_documents')}</p>
              {/if}
              {#if m.error}<p class="err" role="alert"><Icon name="exclamation-triangle" /> {m.error}</p>{/if}
              {#if m.sourcesOnly && !m.citations.length}
                <p class="hint" role="note" id="d2-no-hits"><Icon name="info-circle" /> {t('m.assistant.no_matching_passages_found')}</p>
              {:else if m.sourcesOnly}
                {#if aiOn}
                  <p class="hint" role="note" id="d2-no-answer"><Icon name="info-circle" /> {t('m.assistant.no_written_answer_could')}</p>
                {:else}
                  <p class="hint" role="note" id="d2-ai-off"><Icon name="info-circle" /> {t('m.assistant.ai_off_no_written')}</p>
                {/if}
                <ol class="passages">
                  {#each m.citations as c (c.n)}
                    <li><a href={router.href('documents/' + c.document.id)} class="ptitle"><span class="n">{c.n}</span>{c.document.title}{#if c.document.page}<span class="d2-muted"> · {t('m.assistant.p', { PH: c.document.page })}</span>{/if}</a>{#if c.snippet}<p class="snip">{c.snippet}</p>{/if}</li>
                  {/each}
                </ol>
              {:else if m.citations.length}
                <div class="cites" aria-label={t('m.assistant.sources')}>
                  <span class="d2-label">{t('m.assistant.sources')}</span>
                  <ul>
                    {#each m.citations as c (c.n)}
                      <li>
                        <a class="cite" href={router.href('documents/' + c.document.id)} title={c.snippet ?? ''}>
                          <span class="n">{c.n}</span><span class="ct">{c.document.title}</span>{#if c.document.page}<span class="d2-muted">· {t('m.assistant.p', { PH: c.document.page })}</span>{/if}
                        </a>
                      </li>
                    {/each}
                  </ul>
                </div>
              {/if}
              {#if m.actions.length}
                <div class="acts">
                  {#each m.actions as a, k (k)}
                    <button class="d2-btn sm" onclick={() => act(a)}><Icon name={a.type === 'create_reminder' ? 'calendar-event' : 'box-arrow-up-right'} />{a.label}</button>
                  {/each}
                </div>
              {/if}
              {#if m.notes.length && !m.streaming && (!m.sourcesOnly || aiOn)}<p class="notes d2-muted">{m.notes.join(' ')}</p>{/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
    <form class="composer" onsubmit={(e) => (e.preventDefault(), send())}>
      <label class="d2-sr" for="d2-ask">{t('m.assistant.question')}</label>
      <textarea id="d2-ask" bind:this={input} bind:value={q} rows="1" placeholder={t('m.assistant.ask_question_about_documents')} onkeydown={onkey} maxlength="2000"></textarea>
      {#if busy}
        <button type="button" class="d2-btn" onclick={stop}><Icon name="x-circle" />{t('m.assistant.stop')}</button>
      {:else}
        <button class="d2-btn primary" disabled={!q.trim()} aria-label={t('common.send')}><Icon name="send" /></button>
      {/if}
    </form>
  </section>
</div>

<style>
  .as {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }
  .as.mobile {
    grid-template-columns: minmax(0, 1fr);
  }
  .side {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--d2-border);
    background: var(--d2-surface);
  }
  .list {
    flex: 1;
    padding: var(--d2-s3);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .new {
    margin-bottom: var(--d2-s2);
  }
  .conv {
    width: 100%;
    border: 0;
    background: none;
    text-align: left;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
    padding: 6px var(--d2-s2);
    border-radius: var(--d2-r1);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
  }
  .conv span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .conv:hover {
    background: var(--d2-surface-2);
  }
  .conv.on {
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
  }
  .small {
    font-size: var(--d2-fs-sm);
  }
  .chat {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--d2-bg);
  }
  .chat .d2-pane-head {
    background: var(--d2-surface);
  }
  .chat h1 {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
  }
  .grow {
    flex: 1;
  }
  .demo {
    background: var(--d2-warn-soft);
    color: var(--d2-warn);
  }
  .msgs {
    flex: 1;
    padding: var(--d2-s5) var(--d2-s4);
    display: flex;
    flex-direction: column;
    gap: var(--d2-s4);
  }
  .intro {
    margin: auto;
    max-width: 560px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--d2-s2);
    text-align: center;
    color: var(--d2-accent-text);
  }
  .intro h2 {
    color: var(--d2-text);
  }
  .sugg {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: var(--d2-s2);
    margin-top: var(--d2-s3);
  }
  .sugg .d2-btn {
    height: auto;
    min-height: 32px;
    white-space: normal;
  }
  .warn {
    color: var(--d2-warn);
  }
  .msg {
    display: flex;
    max-width: 820px;
    width: 100%;
    margin: 0 auto;
  }
  .msg.user {
    justify-content: flex-end;
  }
  .bubble {
    max-width: 80%;
    padding: var(--d2-s2) var(--d2-s3);
    border-radius: 14px 14px 4px 14px;
    background: var(--d2-accent);
    color: var(--d2-on-accent);
    white-space: pre-wrap;
  }
  .answer {
    flex: 1;
    padding: var(--d2-s3) var(--d2-s4);
    border: 1px solid var(--d2-border);
    border-radius: 4px 14px 14px 14px;
    background: var(--d2-surface);
  }
  .txt {
    white-space: pre-wrap;
    line-height: 1.6;
  }
  .ref {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    margin: 0 2px;
    padding: 0 4px;
    border: 0;
    border-radius: 9px;
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-size: 11px;
    font-weight: 700;
    vertical-align: super;
    cursor: pointer;
  }
  .caret {
    display: inline-block;
    width: 7px;
    height: 1em;
    margin-left: 2px;
    background: var(--d2-accent);
    vertical-align: text-bottom;
    animation: blink 1s steps(2) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .err {
    margin-top: var(--d2-s2);
    color: var(--d2-danger);
  }
  .cites {
    margin-top: var(--d2-s3);
  }
  .cites ul {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
  }
  .cite {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 320px;
    padding: 4px 10px 4px 4px;
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r-pill);
    color: var(--d2-text);
    font-size: var(--d2-fs-sm);
    text-decoration: none;
  }
  .cite:hover {
    border-color: var(--d2-accent);
  }
  .n {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--d2-accent-soft);
    color: var(--d2-accent-text);
    font-size: 11px;
    font-weight: 700;
  }
  .ct {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .acts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: var(--d2-s3);
  }
  .hint {
    display: flex;
    gap: 6px;
    align-items: flex-start;
    padding: var(--d2-s2) var(--d2-s3);
    border-radius: var(--d2-r1);
    background: var(--d2-info-soft);
    color: var(--d2-text);
    font-size: var(--d2-fs-sm);
    text-align: left;
  }
  .hint :global(.d2-ico) {
    margin-top: 3px;
    color: var(--d2-info);
  }
  .passages {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    margin: var(--d2-s3) 0 0;
    padding: 0;
    list-style: none;
  }
  .passages li {
    padding: var(--d2-s2) var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r1);
  }
  .ptitle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--d2-text);
    font-weight: 600;
    text-decoration: none;
  }
  .ptitle:hover {
    color: var(--d2-accent-text);
    text-decoration: underline;
  }
  .snip {
    margin-top: 4px;
    color: var(--d2-text-2);
    font-size: var(--d2-fs-sm);
  }
  .notes {
    margin-top: var(--d2-s2);
    font-size: var(--d2-fs-xs);
  }
  .composer {
    display: flex;
    align-items: flex-end;
    gap: var(--d2-s2);
    max-width: 852px;
    width: 100%;
    margin: 0 auto;
    padding: var(--d2-s3) var(--d2-s4) var(--d2-s4);
  }
  textarea {
    flex: 1;
    min-height: 44px;
    max-height: 160px;
    padding: 11px var(--d2-s3);
    border: 1px solid var(--d2-border-strong);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
    resize: none;
    field-sizing: content;
  }
  textarea:focus {
    outline: 2px solid var(--d2-accent);
    outline-offset: 0;
  }
  .composer .d2-btn {
    height: 44px;
  }
  .composer .d2-btn.primary {
    width: 44px;
  }
</style>
