import { describe, expect, it } from "vitest";

import { isRoomCodePath, normalizeRoomCode } from "./roomCode";

describe("isRoomCodePath", () => {
  it("accepts normal room codes", () => {
    expect(isRoomCodePath("abc123")).toBe(true);
    expect(isRoomCodePath("my-room-2")).toBe(true);
    expect(isRoomCodePath("666")).toBe(true);
  });

  it("accepts the 3 and 32 character boundaries", () => {
    expect(isRoomCodePath("a".repeat(3))).toBe(true);
    expect(isRoomCodePath("a".repeat(32))).toBe(true);
  });

  it("rejects invalid, wrong-case, and out-of-range codes", () => {
    expect(isRoomCodePath("")).toBe(false);
    expect(isRoomCodePath("ab")).toBe(false);
    expect(isRoomCodePath("a".repeat(33))).toBe(false);
    expect(isRoomCodePath("ABC")).toBe(false);
    expect(isRoomCodePath("a_b")).toBe(false);
    expect(isRoomCodePath("房间号")).toBe(false);
    expect(isRoomCodePath("a b")).toBe(false);
  });
});

describe("normalizeRoomCode", () => {
  it("trims and lowercases normal input", () => {
    expect(normalizeRoomCode("  My-Room-2  ")).toBe("my-room-2");
  });

  it("normalizes boundary-length and whitespace-only input", () => {
    expect(normalizeRoomCode("A".repeat(32))).toBe("a".repeat(32));
    expect(normalizeRoomCode("   ")).toBe("");
    expect(normalizeRoomCode("")).toBe("");
  });

  it("does not validate, only normalize", () => {
    expect(normalizeRoomCode(" A_B ")).toBe("a_b");
    expect(normalizeRoomCode(" 中文 ")).toBe("中文");
  });
});
