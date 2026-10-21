import type { ReactNode } from "react";
import type { Member, Message } from "../api";
import { fmtMsgTime } from "../lib/time";

interface MessageListProps {
  messages: Message[];
  members: Member[];
  onReply: (message: Message) => void;
  onCopy: (body: string) => void;
  onReact: (messageId: string, emoji: string) => void;
}

interface MessageItemProps {
  message: Message;
  members: Member[];
  onReply: (message: Message) => void;
  onCopy: (body: string) => void;
  onReact: (messageId: string, emoji: string) => void;
}

const REACT_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "😡", "🎉"];

/** @昵称 高亮渲染（成员昵称匹配，词边界）。 */
function highlightMentions(text: string, members: Member[]): ReactNode {
  const names = members
    .map((m) => m.nickname || m.username || "")
    .filter((n) => n.length > 0);
  if (names.length === 0) return text;
  const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`@(${names.map(esc).join("|")})(?![\\w-])`, "g");
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(
      <span className="mention" key={m.index}>
        @{m[1]}
      </span>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out.length > 0 ? out : text;
}

/** 消息正文渲染：URL 转链接 + @提及高亮。 */
function renderBody(text: string, members: Member[]): ReactNode {
  const urlRe = /(https?:\/\/[^\s<>"'）)】]+)/g;
  const parts: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = urlRe.exec(text)) !== null) {
    if (m.index > last) parts.push(highlightMentions(text.slice(last, m.index), members));
    const url = m[1];
    parts.push(
      <a key={m.index} href={url} target="_blank" rel="noopener noreferrer" className="msg-link">
        {url}
      </a>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(highlightMentions(text.slice(last), members));
  return parts.length > 0 ? parts : text;
}

export function MessageItem({
  message,
  members,
  onReply,
  onCopy,
  onReact,
}: MessageItemProps) {
  if (!message.sender) {
    return (
      <li className="message system">
        <span className="who system">系统</span>
        <span className="body">{message.body}</span>
      </li>
    );
  }

  return (
    <li className="message">
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
      </span>
      <span className="body">{renderBody(message.body, members)}</span>
      <span className="msg-actions">
        <button className="copy-msg" onClick={() => onReply(message)}>
          回复
        </button>
        <button className="copy-msg" onClick={() => onCopy(message.body)}>
          复制
        </button>
        <span className="reaction-picker">
          {REACT_EMOJIS.map((re) => (
            <button key={re} onClick={() => onReact(message.id, re)}>
              {re}
            </button>
          ))}
        </span>
      </span>
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
}

export function MessageList({ messages, members, onReply, onCopy, onReact }: MessageListProps) {
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
        />
      ))}
    </>
  );
}
