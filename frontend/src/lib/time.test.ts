import { describe, expect, it } from "vitest";

import { fmtAgo, fmtMsgTime } from "./time";

describe("fmtMsgTime", () => {
  const now = new Date(2026, 9, 20, 12, 0, 0);

  it("formats today as HH:MM", () => {
    const message = new Date(2026, 9, 20, 9, 5, 0);
    expect(fmtMsgTime(message.toISOString(), now)).toBe("09:05");
  });

  it("formats dates in the current year without the year", () => {
    const message = new Date(2026, 0, 2, 3, 4, 0);
    expect(fmtMsgTime(message.toISOString(), now)).toBe("1月2日 03:04");
  });

  it("formats previous years with the year", () => {
    const message = new Date(2025, 11, 31, 23, 59, 0);
    expect(fmtMsgTime(message.toISOString(), now)).toBe("2025年12月31日 23:59");
  });

  it("handles the exact current day boundary", () => {
    const message = new Date(2026, 9, 20, 0, 0, 0);
    expect(fmtMsgTime(message.toISOString(), now)).toBe("00:00");
  });

  it("returns an empty string for invalid input", () => {
    expect(fmtMsgTime("not-a-date", now)).toBe("");
  });
});

describe("fmtAgo", () => {
  const now = new Date(2026, 9, 20, 12, 0, 0).getTime();

  it("reports under one minute as just now", () => {
    expect(fmtAgo(new Date(now - 59_999).toISOString(), now)).toBe("刚刚");
  });

  it("reports minute and hour boundaries", () => {
    expect(fmtAgo(new Date(now - 60_000).toISOString(), now)).toBe("1 分钟前");
    expect(fmtAgo(new Date(now - 59 * 60_000).toISOString(), now)).toBe("59 分钟前");
    expect(fmtAgo(new Date(now - 60 * 60_000).toISOString(), now)).toBe("1 小时前");
    expect(fmtAgo(new Date(now - 24 * 60 * 60_000).toISOString(), now)).toBe("1 天前");
  });

  it("clamps future timestamps to just now", () => {
    expect(fmtAgo(new Date(now + 60_000).toISOString(), now)).toBe("刚刚");
  });

  it("returns an empty string for invalid input", () => {
    expect(fmtAgo("not-a-date", now)).toBe("");
  });
});
