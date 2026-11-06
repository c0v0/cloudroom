import type { ReactNode } from "react";
import { assetUrl } from "../api";

/**
 * 轻量 Markdown 渲染（无 HTML 注入）：
 * 代码块 ```lang、行内 `code`、**粗体**、*斜体*、~~删除线~~、链接、@提及。
 * 只返回 React 节点，任何输入都不会被当作 HTML 解析。
 */

let mentionKey = "";
let mentionRe: RegExp | null = null;

function mentionRegex(names: string[]): RegExp | null {
  const key = names.join("\u0000");
  if (key === mentionKey) return mentionRe;
  mentionKey = key;
  if (names.length === 0) {
    mentionRe = null;
    return null;
  }
  const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  mentionRe = new RegExp(`@(${names.map(esc).join("|")})(?![\\w-])`, "g");
  return mentionRe;
}

/** 纯文本里的 @提及高亮。 */
function mentionNodes(text: string, names: string[], key: string): ReactNode[] {
  const re = mentionRegex(names);
  if (!re || !text) return [text];
  re.lastIndex = 0;
  const out: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  let n = 0;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(
      <span className="mention" key={`${key}-m${n++}`}>
        @{m[1]}
      </span>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out.length > 0 ? out : [text];
}

/** 把纯文本节点里的 :name: 替换为自定义 emoji 图片。 */
function withEmojis(
  nodes: ReactNode[],
  emojis: Record<string, string>,
  key: string,
): ReactNode[] {
  if (!emojis || Object.keys(emojis).length === 0) return nodes;
  const re = /:([a-z0-9_+-]{2,32}):/g;
  const out: ReactNode[] = [];
  let n = 0;
  for (const node of nodes) {
    if (typeof node !== "string") {
      out.push(node);
      continue;
    }
    let last = 0;
    let m: RegExpExecArray | null;
    re.lastIndex = 0;
    while ((m = re.exec(node)) !== null) {
      const url = emojis[m[1]];
      if (!url) continue;
      const src = assetUrl(url);
      if (!src) continue;
      if (m.index > last) out.push(node.slice(last, m.index));
      out.push(
        <img
          key={`${key}-e${n++}`}
          className="custom-emoji"
          src={src}
          alt={m[0]}
          title={m[0]}
          loading="lazy"
        />,
      );
      last = m.index + m[0].length;
    }
    if (last < node.length) out.push(node.slice(last));
  }
  return out;
}

const INLINE =
  /(`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|~~[^~\n]+~~|https?:\/\/[^\s<>"'）)】]+)/g;

function renderInline(text: string, names: string[], emojis: Record<string, string>, key: string): ReactNode[] {
  const out: ReactNode[] = [];
  let last = 0;
  let n = 0;
  INLINE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = INLINE.exec(text)) !== null) {
    if (m.index > last) {
      out.push(...withEmojis(mentionNodes(text.slice(last, m.index), names, `${key}-${n}`), emojis, `${key}-${n}`));
    }
    const tok = m[0];
    const k = `${key}-i${n++}`;
    if (tok.startsWith("`")) {
      out.push(
        <code className="md-inline" key={k}>
          {tok.slice(1, -1)}
        </code>,
      );
    } else if (tok.startsWith("**")) {
      out.push(<strong key={k}>{tok.slice(2, -2)}</strong>);
    } else if (tok.startsWith("~~")) {
      out.push(<del key={k}>{tok.slice(2, -2)}</del>);
    } else if (tok.startsWith("*")) {
      out.push(<em key={k}>{tok.slice(1, -1)}</em>);
    } else {
      out.push(
        <a key={k} href={tok} target="_blank" rel="noopener noreferrer" className="msg-link">
          {tok}
        </a>,
      );
    }
    last = m.index + tok.length;
  }
  if (last < text.length) {
    out.push(...withEmojis(mentionNodes(text.slice(last), names, `${key}-${n}`), emojis, `${key}-${n}`));
  }
  return out;
}

/** 渲染消息正文：代码块 + 行内 Markdown + 链接 + @提及。 */
export function renderRichText(
  text: string,
  names: string[],
  emojis: Record<string, string> = {},
): ReactNode {
  const parts: ReactNode[] = [];
  const fence = /```([\w-]*)\n?([\s\S]*?)```/g;
  let last = 0;
  let n = 0;
  let m: RegExpExecArray | null;
  while ((m = fence.exec(text)) !== null) {
    if (m.index > last) {
      parts.push(...renderInline(text.slice(last, m.index), names, emojis, `s${n}`));
    }
    parts.push(
      <pre className="md-code" key={`c${n++}`}>
        <code>{m[2]}</code>
      </pre>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) {
    parts.push(...renderInline(text.slice(last), names, emojis, `s${n}`));
  }
  return parts.length > 0 ? parts : text;
}
