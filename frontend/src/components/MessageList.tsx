import { memo, useMemo, useState } from "react";
import type { ReactNode } from "react";
import type { Attachment, Member, Message } from "../api";
import { assetUrl } from "../api";
import { fmtMsgTime } from "../lib/time";
import { renderRichText } from "../lib/markdown";
import { LinkPreview } from "./LinkPreview";

interface MessageListProps {
  messages: Message[];
  members: Member[];
  onReply: (message: Message) => void;
  onCopy: (body: string) => void;
  onReact: (messageId: string, emoji: string) => void;
  onEdit: (messageId: string, body: string) => Promise<boolean>;
  onDelete: (messageId: string) => void;
  /** 打开线程面板；根消息才显示入口。 */
  onThread?: (message: Message) => void;
  /** 房主固定/取消固定消息。 */
  onPin?: (message: Message) => void;
  /** 转发消息到其他房间。 */
  onForward?: (message: Message) => void;
  /** 自定义 emoji 名称 → 图片 URL。 */
  emojis?: Record<string, string>;
  myUserId?: string;
  /** 成员已读游标（user_id → 最大已读 seq），用于自己消息的「已读 N」。 */
  reads?: Record<string, number>;
  /** 房主/管理员可以删除他人消息。 */
  canModerate?: boolean;
}

interface MessageItemProps extends Omit<MessageListProps, "messages" | "reads" | "members"> {
  message: Message;
  /** 已读人数由列表统一计算，避免每行遍历成员。 */
  readCount: number;
  /** 发送者是否为房主（消息区显示身份徽标）。 */
  isOwner?: boolean;
  /** 仅昵称列表；成员在线状态变化不再让整段消息重渲染。 */
  mentionNames: string[];
}

const FIRST_URL = /https?:\/\/[^\s<>"'）)】]+/;

function firstUrl(body: string): string | null {
  const m = body.match(FIRST_URL);
  return m ? m[0] : null;
}

const REACT_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "😡", "🎉"];

function reactionDisplay(key: string, emojis: Record<string, string>): ReactNode {
  const name = key.startsWith(":") && key.endsWith(":") ? key.slice(1, -1) : null;
  const url = name ? emojis[name] : null;
  if (!url) return key;
  return <img className="custom-emoji" src={assetUrl(url) ?? url} alt={key} title={key} />;
}

/** 消息正文渲染：代码块/粗体/斜体/行内代码/链接 + @提及。 */
function renderBody(
  text: string,
  names: string[],
  emojis: Record<string, string>,
): ReactNode {
  return renderRichText(text, names, emojis);
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function AttachmentView({ attachment }: { attachment: Attachment }) {
  const src = assetUrl(attachment.url) ?? attachment.url;
  const thumb = assetUrl(attachment.thumbnail_url ?? null) ?? src;
  if (attachment.mime.startsWith("image/")) {
    return (
      <a
        className="msg-image-link"
        href={src}
        target="_blank"
        rel="noopener noreferrer"
        title={attachment.filename}
      >
        <img
          className="msg-image"
          src={thumb}
          alt={attachment.filename}
          loading="lazy"
          onError={(e) => {
            const img = e.currentTarget;
            if (img.dataset.fallback === "1") return;
            img.dataset.fallback = "1";
            img.src = src;
          }}
        />
      </a>
    );
  }
  if (attachment.mime.startsWith("audio/")) {
    return (
      <audio
        className="msg-audio"
        controls
        preload="metadata"
        src={src}
        title={attachment.filename}
      />
    );
  }
  if (attachment.mime.startsWith("video/")) {
    return (
      <video
        className="msg-video"
        controls
        preload="metadata"
        src={src}
        title={attachment.filename}
      />
    );
  }
  return (
    <a
      className="msg-file"
      href={src}
      target="_blank"
      download={attachment.filename}
      rel="noopener noreferrer"
    >
      <span className="msg-file-name">{attachment.filename}</span>
      <span className="msg-file-size">{fmtSize(attachment.size)}</span>
    </a>
  );
}

export const MessageItem = memo(function MessageItem({
  message,
  mentionNames,
  onReply,
  onCopy,
  onReact,
  onEdit,
  onDelete,
  onThread,
  onPin,
  onForward,
  emojis,
  myUserId,
  readCount,
  isOwner,
  canModerate,
}: MessageItemProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(message.body);
  const [busy, setBusy] = useState(false);
  // 消息正文只在内容/提及昵称变化时重新解析，已读游标等状态更新不触发。
  const previewUrl = useMemo(() => firstUrl(message.body), [message.body]);
  const bodyNode = useMemo(
    () => renderBody(message.body, mentionNames, emojis ?? {}),
    [message.body, mentionNames, emojis],
  );

  if (!message.sender) {
    return (
      <li className="message system">
        <span className="who system">系统</span>
        <span className="body">{message.body}</span>
      </li>
    );
  }

  const mine = !!myUserId && message.sender.user_id === myUserId;
  const canEdit = mine && !message.deleted && !message.attachment_id;
  const canDelete = !message.deleted && (mine || !!canModerate);
  const shownReadCount = mine ? readCount : 0;

  const save = async () => {
    const body = draft.trim();
    if (!body || body === message.body) {
      setEditing(false);
      return;
    }
    setBusy(true);
    const ok = await onEdit(message.id, body);
    setBusy(false);
    if (ok) setEditing(false);
  };

  return (
    <li className="message" id={`msg-${message.id}`}>
      {message.reply && (
        <span className="reply-quote">
          <b>回复 {message.reply.sender}</b>：{message.reply.body}
        </span>
      )}
      <span className="who">
        {message.sender.nickname ||
          message.sender.username ||
          message.sender.user_id?.slice(0, 8)}
        {isOwner && (
          <span
            className="tag msg-owner"
            title={`账号：@${message.sender.username ?? message.sender.user_id}`}
          >
            房主
          </span>
        )}
        <span className="msg-time">{fmtMsgTime(message.created_at)}</span>
        {message.edited_at && !message.deleted && <span className="msg-edited">已编辑</span>}
      </span>
      {message.forwarded_from && !message.deleted && (
        <span className="forwarded-from">
          转发自 /{message.forwarded_from.room_code} · {message.forwarded_from.sender}
        </span>
      )}
      {message.deleted ? (
        <span className="body msg-deleted">消息已删除</span>
      ) : editing ? (
        <span className="body msg-edit">
          <input
            className="msg-edit-input"
            value={draft}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void save();
              if (e.key === "Escape") setEditing(false);
            }}
          />
          <button disabled={busy} onClick={() => void save()}>
            保存
          </button>
          <button disabled={busy} onClick={() => setEditing(false)}>
            取消
          </button>
        </span>
      ) : (
        <span className="body">{bodyNode}</span>
      )}
      {!message.deleted && !editing && previewUrl && <LinkPreview url={previewUrl} />}
      {mine && !message.deleted && shownReadCount > 0 && (
        <span className="read-count" title={`${shownReadCount} 人已读`}>
          已读 {shownReadCount}
        </span>
      )}
      {!editing && !message.deleted && message.attachment && (
        <span className="body msg-attachment">
          <AttachmentView attachment={message.attachment} />
        </span>
      )}
      {!message.deleted && (
        <span className="msg-actions">
          <button className="copy-msg" onClick={() => onReply(message)}>
            回复
          </button>
          {onThread && message.sender?.user_id && !message.thread_root_id && (
            <button className="copy-msg" onClick={() => onThread(message)}>
              线程
              {(message.thread_reply_count ?? 0) > 0 ? ` ${message.thread_reply_count}` : ""}
              {(message.thread_unread ?? 0) > 0 && (
                <span className="thread-unread">{message.thread_unread}</span>
              )}
            </button>
          )}
          <button className="copy-msg" onClick={() => onCopy(message.body)}>
            复制
          </button>
          {onPin && (
            <button className="copy-msg" onClick={() => onPin(message)}>
              {message.pinned ? "取消固定" : "固定"}
            </button>
          )}
          {onForward && (
            <button className="copy-msg" onClick={() => onForward(message)}>
              转发
            </button>
          )}
          {canEdit && (
            <button
              className="copy-msg"
              onClick={() => {
                setDraft(message.body);
                setEditing(true);
              }}
            >
              编辑
            </button>
          )}
          {canDelete && (
            <button className="copy-msg danger" onClick={() => onDelete(message.id)}>
              删除
            </button>
          )}
          <span className="reaction-picker">
            {REACT_EMOJIS.map((re) => (
              <button key={re} onClick={() => onReact(message.id, re)}>
                {re}
              </button>
            ))}
            {Object.entries(emojis ?? {}).slice(0, 8).map(([name, url]) => (
              <button key={name} onClick={() => onReact(message.id, ":" + name + ":")}>
                <img className="custom-emoji" src={assetUrl(url) ?? url} alt={":" + name + ":"} />
              </button>
            ))}
          </span>
        </span>
      )}
      {(message.reactions?.length ?? 0) > 0 && (
        <span className="reaction-row">
          {message.reactions!.map((r) => (
            <button
              key={r.emoji}
              className={r.me ? "me" : ""}
              onClick={() => onReact(message.id, r.emoji)}
            >
              {reactionDisplay(r.emoji, emojis ?? {})} {r.count}
            </button>
          ))}
        </span>
      )}
    </li>
  );
});

/**
 * 消息列表按 props 浅比较缓存：说话动效每 120ms 更新 speaking，若父组件重渲染
 * 会连带重排整段消息，这里把这块子树隔离掉（回调需由父组件 memo 化）。
 */
export const MessageList = memo(function MessageList({
  messages,
  members,
  onReply,
  onCopy,
  onReact,
  onEdit,
  onDelete,
  onThread,
  onPin,
  onForward,
  emojis,
  myUserId,
  reads,
  canModerate,
}: MessageListProps) {
  // 已读人数：把其他成员的已读游标排序一次，逐条消息用二分计数，
  // 避免每条消息 × 每个成员的全量扫描。
  const otherReads = useMemo(() => {
    const out: number[] = [];
    for (const m of members) {
      if (m.user_id === myUserId) continue;
      out.push(reads?.[m.user_id] ?? 0);
    }
    out.sort((a, b) => a - b);
    return out;
  }, [members, reads, myUserId]);

  const mentionKey = members
    .map((m) => m.nickname || m.username || "")
    .filter(Boolean)
    .join("\u0000");
  const mentionNames = useMemo(
    () => (mentionKey ? mentionKey.split("\u0000") : []),
    [mentionKey],
  );

  // 房主身份徽标：昵称可重复，但 user_id 唯一。
  const ownerId = useMemo(
    () => members.find((m) => m.room_role === "owner")?.user_id,
    [members],
  );

  const readCountFor = (seq: number) => {
    let lo = 0;
    let hi = otherReads.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (otherReads[mid] < seq) lo = mid + 1;
      else hi = mid;
    }
    return otherReads.length - lo;
  };

  return (
    <>
      {messages.map((message) => (
        <MessageItem
          key={message.id}
          message={message}
          mentionNames={mentionNames}
          onReply={onReply}
          onCopy={onCopy}
          onReact={onReact}
          onEdit={onEdit}
          onDelete={onDelete}
          onThread={onThread}
          onPin={onPin}
          onForward={onForward}
          emojis={emojis}
          myUserId={myUserId}
          canModerate={canModerate}
          isOwner={!!ownerId && message.sender?.user_id === ownerId}
          readCount={
            myUserId && message.sender?.user_id === myUserId
              ? readCountFor(message.seq)
              : 0
          }
        />
      ))}
    </>
  );
});
