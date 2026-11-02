import type { Message } from "../api";

/**
 * Append incoming messages, preserving order and ignoring duplicate ids.
 * Returns the original array when nothing was added.
 */
export function mergeMessages(prev: Message[], incoming: Message[]): Message[] {
  if (incoming.length === 0) return prev;
  const seen = new Set(prev.map((message) => message.id));
  const next = prev.slice();
  for (const message of incoming) {
    if (!message?.id || seen.has(message.id)) continue;
    seen.add(message.id);
    next.push(message);
  }
  return next;
}

/**
 * Prepend an older page, keeping chronological order and dropping duplicates.
 * Returns the original array when nothing was added.
 */
export function mergeOlder(older: Message[], current: Message[]): Message[] {
  if (older.length === 0) return current;
  const seen = new Set(current.map((m) => m.id));
  const head = older.filter((m) => m?.id && !seen.has(m.id));
  if (head.length === 0) return current;
  return [...head, ...current];
}
