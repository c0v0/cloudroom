import type { Message } from "../api";

/** 按服务端 seq 排序：断线补齐/搜索结果/实时消息可以乱序到达。 */
function bySeq(a: Message, b: Message): number {
  return a.seq - b.seq;
}

/**
 * 合并消息：按 id 去重、按 seq 排序，并接受同一条消息的更新
 * （编辑、软删除、回应聚合）。返回原数组表示没有变化。
 */
export function mergeMessages(prev: Message[], incoming: Message[]): Message[] {
  if (incoming.length === 0) return prev;
  const byId = new Map(prev.map((message) => [message.id, message]));
  let changed = false;
  for (const message of incoming) {
    if (!message?.id) continue;
    const old = byId.get(message.id);
    if (!old) {
      byId.set(message.id, message);
      changed = true;
      continue;
    }
    if (
      old.body !== message.body ||
      old.edited_at !== message.edited_at ||
      old.deleted !== message.deleted ||
      old.reactions !== message.reactions ||
      old.attachment !== message.attachment
    ) {
      byId.set(message.id, message);
      changed = true;
    }
  }
  if (!changed) return prev;
  return [...byId.values()].sort(bySeq);
}

/**
 * Prepend an older page, keeping chronological order and dropping duplicates.
 * Returns the original array when nothing was added.
 */
export function mergeOlder(older: Message[], current: Message[]): Message[] {
  if (older.length === 0) return current;
  const merged = mergeMessages(current, older);
  return merged === current ? current : merged;
}
