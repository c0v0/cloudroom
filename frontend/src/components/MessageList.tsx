import { memo, useState } from "react";
import type { ReactNode } from "react";
import type { Attachment, Member, Message } from "../api";
import { assetUrl } from "../api";
import { fmtMsgTime } from "../lib/time";
import { renderRichText } from "../lib/markdown";

interface MessageListProps {
  messages: Message[];
  members: Member[];
  onReply: (message: Message) => void;
  onCopy: (body: string) => void;
  onReact: (messageId: string, emoji: string) => void;
  onEdit: (messageId: string, body: string) => Promise<boolean>;
  onDelete: (messageId: string) => void;
  myUserId?: string;
  /** 成员已读游标（user_id → 最大已读 seq），用于自己消息的「已读 N」。 */
  reads?: Record<string, number>;
  /** 房主/管理员可以删除他人消息。 */
  canModerate?: boolean;
}

interface MessageItemProps extends Omit<MessageListProps, "messages"> {
  message: Message;
}

const REACT_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "😡", "🎉"];

/** 消息正文渲染：代码块/粗体/斜体/行内代码/链接 + @提及。 */
function renderBody(text: string, members: Member[]): ReactNode {
  return renderRichText(text, members);
}

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function AttachmentView({ attachment }: { attachment: Attachment }) {
  const src = assetUrl(attachment.url) ?? attachment.url;
  if (attachment.mime.startsWith("image/")) {
    return (
      <a
        className="msg-image-link"
        href={src}
        target="_blank"
        rel="noopener noreferrer"
        title={attachment.filename}
      >
        <img className="msg-image" src={src} alt={attachment.filename} loading="lazy" />
      </a>
    );
  }
  return (
    <a className="msg-file" href={src} download={attachment.filename} rel="noopener noreferrer">
      <span className="msg-file-name">{attachment.filename}</span>
      <span className="msg-file-size">{fmtSize(attachment.size)}</span>
    </a>
  );
}

export const MessageItem = memo(function MessageItem({
  message,
  members,
  onReply,
  onCopy,
  onReact,
  onEdit,
  onDelete,
  myUserId,
  reads,
  canModerate,
}: MessageItemProps) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(message.body);
  const [busy, setBusy] = useState(false);

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
  const readCount = mine
    ? members.filter(
        (m) => m.user_id !== myUserId && (reads?.[m.user_id] ?? 0) >= message.seq,
      ).length
    : 0;

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
        <span className="msg-time">{fmtMsgTime(message.created_at)}</span>
        {message.edited_at && !message.deleted && <span className="msg-edited">已编辑</span>}
      </span>
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
        <span className="body">{renderBody(message.body, members)}</span>
      )}
      {mine && !message.deleted && readCount > 0 && (
        <span className="read-count" title={`${readCount} 人已读`}>
          已读 {readCount}
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
          <button className="copy-msg" onClick={() => onCopy(message.body)}>
            复制
          </button>
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
              {r.emoji} {r.count}
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
  myUserId,
  reads,
  canModerate,
}: MessageListProps) {
  return (
    <>
      {messages.map((message) => (
        <MessageItem
          key={message.id}
          message={message}
          members={members}
          onReply={onReply}
          onCopy={onCopy}
          onReact={onReact}
          onEdit={onEdit}
          onDelete={onDelete}
          myUserId={myUserId}
          canModerate={canModerate}
          reads={reads}
        />
      ))}
    </>
  );
});
