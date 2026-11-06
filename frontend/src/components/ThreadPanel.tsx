import { FormEvent, useEffect, useState } from "react";
import type { Message, Member } from "../api";
import { api } from "../api";
import { renderRichText } from "../lib/markdown";
import { fmtMsgTime } from "../lib/time";
import { AttachmentView } from "./MessageList";
import { LinkPreview } from "./LinkPreview";

interface ThreadPanelProps {
  code: string;
  rootId: string;
  members: Member[];
  emojis?: Record<string, string>;
  onClose: () => void;
  /** Called after a reply is sent so the room list can refresh reply counts. */
  onSent: () => void;
  /** Bumped by the room socket when a new reply arrives for this thread. */
  refreshKey: number;
  /** Clear the root's unread badge after the thread is marked read. */
  onRead: (rootId: string) => void;
}

/** Side-panel thread view: root message + replies + reply composer. */
export function ThreadPanel({
  code,
  rootId,
  members,
  emojis = {},
  onClose,
  onSent,
  refreshKey,
  onRead,
}: ThreadPanelProps) {
  const [root, setRoot] = useState<Message | null>(null);
  const [replies, setReplies] = useState<Message[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);

  const load = async () => {
    try {
      const r = await api<{ root: Message; replies: Message[] }>(
        `/api/v1/rooms/${code}/messages/${encodeURIComponent(rootId)}/thread?limit=100`,
      );
      setRoot(r.root);
      setReplies(r.replies);
      setError(null);
      // Best-effort: mark the newest reply as read; failure must not hide the thread.
      try {
        await api(`/api/v1/rooms/${code}/messages/${encodeURIComponent(rootId)}/thread/read`, {
          method: "POST",
          body: "{}",
        });
        onRead(rootId);
      } catch {
        /* read cursor is non-critical */
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, rootId, refreshKey]);

  const send = async (e: FormEvent) => {
    e.preventDefault();
    const body = draft.trim();
    if (!body || sending) return;
    setSending(true);
    setError(null);
    try {
      await api(`/api/v1/rooms/${code}/messages`, {
        method: "POST",
        body: JSON.stringify({ body, thread_root_id: rootId, reply_to: rootId }),
      });
      setDraft("");
      await load();
      onSent();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSending(false);
    }
  };

  const names = members.map((m) => m.nickname || m.username || "").filter(Boolean);

  const renderMessage = (m: Message, isRoot = false) => (
    <article key={m.id} className={`thread-msg${isRoot ? " thread-root" : ""}`}>
      <div className="thread-meta">
        <b>{m.sender?.nickname || m.sender?.username || "成员"}</b>
        <span className="muted">{fmtMsgTime(m.created_at)}</span>
      </div>
      {m.deleted ? (
        <p className="muted">该消息已删除</p>
      ) : (
        <div className="thread-body">{renderRichText(m.body, names, emojis)}</div>
      )}
      {!m.deleted && (() => { const u = m.body.match(/https?:\/\/[^\s<>"'）)】]+/); return u ? <LinkPreview url={u[0]} /> : null; })()}
      {!m.deleted && m.attachment && (
        <div className="msg-attachment">
          <AttachmentView attachment={m.attachment} />
        </div>
      )}
    </article>
  );

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal thread-modal"
        role="dialog"
        aria-modal="true"
        aria-label="线程"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2>线程</h2>
          <button className="modal-close" aria-label="关闭线程" onClick={onClose}>
            ×
          </button>
        </div>
        {loading && <p className="hint">加载中…</p>}
        {error && <p className="error">{error}</p>}
        <div className="thread-list">
          {root && renderMessage(root, true)}
          {replies.map((m) => renderMessage(m))}
          {!loading && !root && <p className="hint">线程不存在或已删除。</p>}
        </div>
        <form className="thread-compose" onSubmit={send}>
          <textarea
            value={draft}
            autoFocus
            placeholder="回复线程（Enter 发送，Shift+Enter 换行）"
            maxLength={4000}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
          />
          <button disabled={sending || !draft.trim()}>{sending ? "发送中…" : "回复"}</button>
        </form>
      </div>
    </div>
  );
}
