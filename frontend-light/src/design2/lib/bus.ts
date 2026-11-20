// Small command bus: frame (keyboard, palette) → active area (workspace).
import type { Command } from './keys'

type Handler = (cmd: Command) => boolean | void
const handlers = new Set<Handler>()

/** Area registers; return = unregister. Handler returns true if it handled the command. */
export function onCommand(h: Handler): () => void {
  handlers.add(h)
  return () => handlers.delete(h)
}

export function dispatch(cmd: Command): boolean {
  for (const h of [...handlers].reverse()) if (h(cmd)) return true
  return false
}
