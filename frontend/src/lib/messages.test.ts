import { describe, expect, it } from "vitest";

import type { Message } from "../api";
import { mergeMessages } from "./messages";

function message(id: string, seq: number): Message {
  return {
    id,
    seq,
    sender: null,
    body: `body-${id}`,
    created_at: "2026-10-20T00:00:00Z",
  };
}

describe("mergeMessages", () => {
  it("appends new messages in order", () => {
    const first = message("a", 1);
    const second = message("b", 2);
    const merged = mergeMessages([first], [second]);
    expect(merged.map((m) => m.id)).toEqual(["a", "b"]);
    expect(merged[0]).toBe(first);
    expect(merged[1]).toBe(second);
  });

  it("ignores duplicates already in the list or repeated in the batch", () => {
    const first = message("a", 1);
    const second = message("b", 2);
    const merged = mergeMessages([first], [first, second, second]);
    expect(merged.map((m) => m.id)).toEqual(["a", "b"]);
  });

  it("returns the original array for an empty batch", () => {
    const prev = [message("a", 1)];
    expect(mergeMessages(prev, [])).toBe(prev);
  });

  it("skips malformed entries with a missing id", () => {
    const first = message("a", 1);
    const emptyId = { ...message("", 2) };
    const nullish = null as unknown as Message;
    const merged = mergeMessages([first], [emptyId, nullish, message("b", 3)]);
    expect(merged.map((m) => m.id)).toEqual(["a", "b"]);
  });
});
