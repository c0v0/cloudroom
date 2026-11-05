import { useEffect, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { useNavigate } from "react-router-dom";

import { api, Room } from "../api";

interface SearchHit {
  message_id: string;
  room_code: string;
  seq: number;
  body: string;
  created_at: string;
  sender_nickname?: string | null;
}

interface PaletteItem {
  key: string;
  kind: "room" | "message";
  label: string;
  detail: string;
  code: string;
  messageId?: string;
  seq?: number;
}

interface Props {
  open: boolean;
  onClose: () => void;
}

/** Ctrl/Cmd+K 全局搜索：房间跳转 + 跨房间消息（复用 /api/v1/search）。 */
export function CommandPalette({ open, onClose }: Props) {
  const navigate = useNavigate();
  const inputRef = useRef<HTMLInputElement>(null);
  const timerRef = useRef<number | null>(null);
  const searchSeq = useRef(0);
  const [q, setQ] = useState("");
  const [rooms, setRooms] = useState<Room[]>([]);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState(0);

  useEffect(() => {
    if (!open) return;
    setQ("");
    setHits([]);
    setError(null);
    setActive(0);
    void api<{ rooms: Room[] }>("/api/v1/me/rooms")
      .then((r) => setRooms(r.rooms))
      .catch(() => setRooms([]));
    const id = window.setTimeout(() => inputRef.current?.focus(), 0);
    return () => window.clearTimeout(id);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const needle = q.trim();
    if (timerRef.current) window.clearTimeout(timerRef.current);
    const seq = ++searchSeq.current;
    if (needle.length < 2) {
      setHits([]);
      setBusy(false);
      setError(null);
      return;
    }
    setBusy(true);
    timerRef.current = window.setTimeout(() => {
      void api<{ results: SearchHit[] }>(
        `/api/v1/search?q=${encodeURIComponent(needle)}&limit=20`,
      )
        .then((r) => {
          if (seq !== searchSeq.current) return;
          setHits(r.results);
          setError(null);
        })
        .catch((err) => {
          if (seq !== searchSeq.current) return;
          setHits([]);
          setError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          if (seq === searchSeq.current) setBusy(false);
        });
    }, 220);
    return () => {
      if (timerRef.current) window.clearTimeout(timerRef.current);
    };
  }, [q, open]);

  const items = useMemo<PaletteItem[]>(() => {
    const needle = q.trim().toLowerCase();
    const roomItems = rooms
      .filter((r) => !needle || r.code.toLowerCase().includes(needle))
      .slice(0, 6)
      .map<PaletteItem>((r) => ({
        key: `room:${r.code}`,
        kind: "room",
        label: r.code,
        detail: r.kind === "direct" ? "私聊" : "房间",
        code: r.code,
      }));
    const messageItems = hits.map<PaletteItem>((h) => ({
      key: `msg:${h.message_id}`,
      kind: "message",
      label: h.body,
      detail: `${h.room_code} · ${h.sender_nickname || "成员"}`,
      code: h.room_code,
      messageId: h.message_id,
      seq: h.seq,
    }));
    return [...roomItems, ...messageItems];
  }, [rooms, hits, q]);

  useEffect(() => {
    setActive((i) => (items.length === 0 ? 0 : Math.min(i, items.length - 1)));
  }, [items.length]);

  if (!open) return null;

  const openItem = (item: PaletteItem) => {
    onClose();
    if (item.kind === "message" && item.messageId) {
      navigate(`/${item.code}?msg=${item.messageId}&seq=${item.seq ?? 0}`);
    } else {
      navigate(`/${item.code}`);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => (items.length ? (i + 1) % items.length : 0));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => (items.length ? (i - 1 + items.length) % items.length : 0));
    } else if (e.key === "Enter" && items[active]) {
      e.preventDefault();
      openItem(items[active]);
    }
  };

  return (
    <div
      className="palette-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="palette" role="dialog" aria-modal="true" aria-label="全局搜索">
        <input
          ref={inputRef}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="搜索房间或消息…"
          maxLength={100}
          aria-label="全局搜索关键词"
        />
        <div className="palette-results">
          {items.length === 0 && !busy && (
            <p className="hint">{q.trim().length < 2 ? "输入房间号或至少 2 个字符搜索消息。" : "没有匹配结果。"}</p>
          )}
          {busy && <p className="hint">搜索中…</p>}
          {error && <p className="error">{error}</p>}
          {items.map((item, i) => (
            <button
              key={item.key}
              type="button"
              className={`palette-item${i === active ? " active" : ""}`}
              onMouseEnter={() => setActive(i)}
              onClick={() => openItem(item)}
            >
              <span className={`palette-kind ${item.kind}`}>
                {item.kind === "room" ? "房间" : "消息"}
              </span>
              <span className="palette-label">{item.label}</span>
              <span className="palette-detail">{item.detail}</span>
            </button>
          ))}
        </div>
        <div className="palette-foot">
          <span>↑↓ 选择 · Enter 打开 · Esc 关闭</span>
          <span>Ctrl/⌘ K</span>
        </div>
      </div>
    </div>
  );
}
