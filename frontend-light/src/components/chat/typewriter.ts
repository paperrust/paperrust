// Typewriter pacing for streamed chat answers (port of chat.component.ts, upstream 3.5.1 / PR #14805):
// every received burst re-arms a deadline 0.75–3 s ahead (1 ms per queued character), and each 16 ms tick
// reveals an even share of the queued text so the queue is empty when the deadline is reached.

export const TYPEWRITER_MIN_DURATION_MS = 750
export const TYPEWRITER_MAX_DURATION_MS = 3000
export const TYPEWRITER_INTERVAL_MS = 16

/** Reveal duration for a queue of `queued` characters (clamped to 0.75–3 s). */
export function typewriterRevealDuration(queued: number): number {
  return Math.min(TYPEWRITER_MAX_DURATION_MS, Math.max(TYPEWRITER_MIN_DURATION_MS, queued))
}

/** Characters to show on this tick: queue spread over the ticks left until `deadline` (at least one tick). */
export function typewriterChunkSize(queued: number, deadline: number, now: number): number {
  const remainingUpdates = Math.max(1, Math.ceil((deadline - now) / TYPEWRITER_INTERVAL_MS))
  return Math.ceil(queued / remainingUpdates)
}
