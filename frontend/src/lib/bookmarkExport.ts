import type { BookmarkEntry } from "../api";

export interface BookmarkExport {
  exported_at: string;
  bookmarks: BookmarkEntry[];
}

const CSV_HEADER = [
  "message_id",
  "room_code",
  "seq",
  "sender",
  "created_at",
  "body",
  "tags",
];

function csvCell(value: string | number): string {
  return `"${String(value).replaceAll('"', '""')}"`;
}

/** Excel-friendly UTF-8 CSV (BOM + CRLF). */
export function bookmarksToCsv(bookmarks: BookmarkEntry[]): string {
  const rows = bookmarks.map((b) =>
    [
      b.message_id,
      b.room_code,
      b.seq,
      b.sender_nickname || b.sender_username || "",
      b.created_at,
      b.body,
      (b.tags ?? []).join("|"),
    ]
      .map(csvCell)
      .join(","),
  );
  return "\uFEFF" + [CSV_HEADER.map(csvCell).join(","), ...rows].join("\r\n") + "\r\n";
}

/** Markdown list; message bodies are blockquoted so raw markup cannot break layout. */
export function bookmarksToMarkdown(
  bookmarks: BookmarkEntry[],
  exportedAt: string,
): string {
  const lines = ["# 我的收藏", "", `导出时间：${exportedAt}`, ""];
  for (const b of bookmarks) {
    const sender = b.sender_nickname || b.sender_username || "成员";
    lines.push(`## /${b.room_code} · ${sender} · ${b.created_at}`, "");
    if (b.tags && b.tags.length > 0) {
      lines.push(`标签：${b.tags.join("、")}`, "");
    }
    const body = b.body.split(/\r?\n/).map((line) => `> ${line}`).join("\n");
    lines.push(body, "");
  }
  return lines.join("\n");
}

export function bookmarkFilename(format: "json" | "csv" | "md", date = new Date()): string {
  const stamp = date.toISOString().slice(0, 10).replaceAll("-", "");
  return `cloudroom-bookmarks-${stamp}.${format}`;
}
