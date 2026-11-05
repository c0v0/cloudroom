import { describe, expect, it } from "vitest";

import type { BookmarkEntry } from "../api";
import {
  bookmarkFilename,
  bookmarksToCsv,
  bookmarksToMarkdown,
} from "./bookmarkExport";

const entry: BookmarkEntry = {
  message_id: "m1",
  room_code: "room",
  seq: 3,
  body: 'hello, "world"\nnext line',
  created_at: "2026-11-05T00:00:00Z",
  sender_username: "alice",
  sender_nickname: "Alice",
};

describe("bookmark export", () => {
  it("writes BOM CSV with escaped quotes and newlines", () => {
    const csv = bookmarksToCsv([entry]);
    expect(csv.startsWith("\uFEFF")).toBe(true);
    expect(csv).toContain('"hello, ""world""\nnext line"');
    expect(csv).toContain('"m1","room","3","Alice"');
  });

  it("writes Markdown with blockquoted bodies", () => {
    const md = bookmarksToMarkdown([entry], "2026-11-05T00:00:00Z");
    expect(md).toContain("# 我的收藏");
    expect(md).toContain("## /room · Alice · 2026-11-05T00:00:00Z");
    expect(md).toContain('> hello, "world"\n> next line');
  });

  it("uses the export format in the filename", () => {
    expect(bookmarkFilename("csv", new Date("2026-11-05T12:00:00Z"))).toBe(
      "cloudroom-bookmarks-20261105.csv",
    );
  });
});
