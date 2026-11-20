<!-- "Question about this document" (docs/DESIGN2-ENTWIRREN.md §5): button + expandable area in the document view.
     Uses POST /rs/api/v2/ask with `documents: [uuid]` (retrieval restricted to this document, pl-api-v2 ask.rs).
     Only with AI on. Embedding: <AskDocument doc={doc} /> (doc = v1 document with id, title, created). Self-contained,
     no dependency on the surrounding form. -->
<script lang="ts">
  import { tick } from 'svelte'
  import { t } from '../../lib/i18n'
  import { router } from '../../router.svelte'
  import { session, SETTINGS_KEYS } from '../../stores/settings.svelte'
  import { ask, Unavailable, type Citation } from '../api/assist'
  import { resolveDoc } from '../api/v2docs'
  import Icon from '../components/Icon.svelte'

  let { doc }: { doc: { id: number; title: string; created: string } } = $props()

  interface Turn {
    question: string
    text: string
    citations: Citation[]
    error: string | null
    streaming: boolean
  }

  const aiOn = $derived(!!session.get(SETTINGS_KEYS.AI_ENABLED))
  let open = $state(false)
  let q = $state('')
  let turns = $state<Turn[]>([])
  let busy = $state(false)
  let conversation = $state<string | null>(null)
  let uuid: string | null = null
  let docId = 0
  let ctrl: AbortController | null = null
  let input = $state<HTMLTextAreaElement>()

  // other document → reset area
  $effect(() => {
    if (doc.id !== docId) {
      ctrl?.abort()
      docId = doc.id
      uuid = null
      conversation = null
      turns = []
      q = ''
    }
  })
  $effect(() => () => ctrl?.abort())

  async function toggle() {
    open = !open
    if (open) {
      await tick()
      input?.focus()
    }
  }

  function parts(text: string): (string | number)[] {
    return text.split(/\[(\d+)\]/).map((p, i) => (i % 2 ? +p : p))
  }

  async function send() {
    const question = q.trim()
    if (!question || busy) return
    q = ''
    turns = [...turns, { question, text: '', citations: [], error: null, streaming: true }]
    const i = turns.length - 1
    const upd = (f: (x: Turn) => void) => {
      const x = { ...turns[i] }
      f(x)
      turns[i] = x
    }
    busy = true
    ctrl = new AbortController()
    try {
      uuid ??= (await resolveDoc(doc))?.uuid ?? null
      if (!uuid) {
        upd((x) => (x.error = t('m.assistant.document_not_available')))
        return
      }
      await ask(
        question,
        conversation,
        (e) => {
          if (e.type === 'meta' && e.conversation) conversation = e.conversation
          else if (e.type === 'delta') upd((x) => (x.text += e.text))
          else if (e.type === 'citations') upd((x) => (x.citations = e.citations))
          else if (e.type === 'error') upd((x) => (x.error = e.message))
          else if (e.type === 'done')
            upd((x) => {
              if (!x.text && e.answer) x.text = e.answer
              if (e.answerError) x.error = e.answerError
              if (!x.text && !x.error) x.error = t('m.assistant.no_matching_passages_found')
            })
        },
        ctrl.signal,
        [uuid],
      )
    } catch (e: any) {
      if (e instanceof Unavailable) upd((x) => (x.error = t('m.assistant.assistant_not_available_server')))
      else if (e?.name !== 'AbortError') upd((x) => (x.error = e?.message ?? String(e)))
    } finally {
      upd((x) => (x.streaming = false))
      busy = false
      ctrl = null
    }
  }

  function onkey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault()
      send()
    }
  }
</script>

{#if aiOn}
  <div class="askdoc">
    <button class="d2-btn sm" id="d2-ask-doc" aria-expanded={open} aria-controls="d2-ask-doc-panel" onclick={toggle}>
      <Icon name="stars" />{t('m.assistant.ask_about_document')}
    </button>
    {#if open}
      <section class="panel" id="d2-ask-doc-panel" aria-label={t('m.assistant.ask_about_document')}>
        <p class="d2-muted small">{t('m.assistant.answer_only_this_document')}</p>
        {#each turns as x, i (i)}
          <div class="turn" aria-live="polite" aria-busy={x.streaming}>
            <p class="qq">{x.question}</p>
            {#if x.text}
              <p class="txt">
                {#each parts(x.text) as p, j (j)}
                  {#if typeof p === 'number'}
                    {@const c = x.citations.find((c) => c.n === p)}
                    {#if c?.document.page}<span class="ref" title={c.snippet ?? ''}>{t('m.assistant.p', { PH: c.document.page })}</span>{:else}<sup>[{p}]</sup>{/if}
                  {:else}{p}{/if}
                {/each}
              </p>
            {:else if x.streaming}
              <p class="d2-muted small">{t('m.assistant.searching_documents')}</p>
            {/if}
            {#if x.error}<p class="err" role="alert"><Icon name="exclamation-triangle" /> {x.error}</p>{/if}
          </div>
        {/each}
        <form class="composer" onsubmit={(e) => (e.preventDefault(), send())}>
          <label class="d2-sr" for="d2-ask-doc-q">{t('m.assistant.question')}</label>
          <textarea id="d2-ask-doc-q" class="d2-input" bind:this={input} bind:value={q} rows="2" maxlength="2000" placeholder={t('m.assistant.ask_document_placeholder')} onkeydown={onkey}></textarea>
          <div class="row">
            {#if conversation}
              <a class="d2-btn ghost sm" href={router.href('assistant')}><Icon name="stars" />{t('common.assistant')}</a>
            {/if}
            <span class="grow"></span>
            {#if busy}
              <button type="button" class="d2-btn sm" onclick={() => ctrl?.abort()}><Icon name="x-circle" />{t('m.assistant.stop')}</button>
            {:else}
              <button class="d2-btn primary sm" disabled={!q.trim()}><Icon name="send" />{t('common.send')}</button>
            {/if}
          </div>
        </form>
      </section>
    {/if}
  </div>
{/if}

<style>
  .askdoc {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--d2-s2);
    min-width: 0;
  }
  .panel {
    align-self: stretch;
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
    padding: var(--d2-s3);
    border: 1px solid var(--d2-border);
    border-radius: var(--d2-r2);
    background: var(--d2-surface);
  }
  .small {
    margin: 0;
    font-size: var(--d2-fs-sm);
  }
  .turn {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .qq {
    margin: 0;
    font-weight: 600;
  }
  .txt {
    margin: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .ref {
    margin: 0 2px;
    padding: 0 4px;
    border-radius: var(--d2-r1);
    background: var(--d2-surface-2);
    color: var(--d2-text-2);
    font-size: var(--d2-fs-xs);
  }
  .err {
    margin: 0;
    color: var(--d2-danger);
    font-size: var(--d2-fs-sm);
  }
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--d2-s2);
  }
  .composer textarea {
    resize: vertical;
    min-height: 3.2em;
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--d2-s2);
  }
  .grow {
    flex: 1;
  }
</style>
