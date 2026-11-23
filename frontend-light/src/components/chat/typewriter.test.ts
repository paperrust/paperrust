import { describe, expect, it } from 'vitest'
import { TYPEWRITER_INTERVAL_MS, TYPEWRITER_MAX_DURATION_MS, TYPEWRITER_MIN_DURATION_MS, typewriterChunkSize, typewriterRevealDuration } from './typewriter'

describe('typewriter pacing (upstream 3.5.1)', () => {
  it('reveal duration is 1 ms per character, clamped to 0.75-3 s', () => {
    expect(typewriterRevealDuration(1)).toBe(TYPEWRITER_MIN_DURATION_MS)
    expect(typewriterRevealDuration(1200)).toBe(1200)
    expect(typewriterRevealDuration(50000)).toBe(TYPEWRITER_MAX_DURATION_MS)
  })
  it('spreads the queue evenly over the ticks left', () => {
    // 750 ms left = 47 ticks: 94 chars -> 2 per tick
    expect(typewriterChunkSize(94, 1750, 1000)).toBe(2)
    // fewer chars than ticks: at least one per tick
    expect(typewriterChunkSize(3, 1750, 1000)).toBe(1)
    expect(typewriterChunkSize(0, 1750, 1000)).toBe(0)
  })
  it('flushes everything once the deadline has passed', () => {
    expect(typewriterChunkSize(500, 1000, 1000)).toBe(500)
    expect(typewriterChunkSize(500, 1000, 5000)).toBe(500)
  })
  it('drains a burst within its deadline', () => {
    let queued = 2000
    let now = 0
    const deadline = now + typewriterRevealDuration(queued)
    let ticks = 0
    while (queued > 0) {
      queued -= typewriterChunkSize(queued, deadline, now)
      now += TYPEWRITER_INTERVAL_MS
      ticks++
    }
    expect(ticks).toBeLessThanOrEqual(Math.ceil(2000 / TYPEWRITER_INTERVAL_MS) + 1)
    expect(ticks).toBeGreaterThan(100)
  })
})
