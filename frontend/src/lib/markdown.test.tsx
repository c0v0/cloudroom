import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { Member } from "../api";
import { renderRichText } from "./markdown";

const html = (text: string, members: Member[] = []) =>
  renderToStaticMarkup(<>{renderRichText(text, members.map((m) => m.nickname))}</>);

const member = (nickname: string) => ({ user_id: "u1", nickname } as Member);

describe("renderRichText", () => {
  it("renders inline markdown and fenced code", () => {
    const out = html("**bold** *it* `code`\n```js\nconst a = 1;\n```");
    expect(out).toContain("<strong>bold</strong>");
    expect(out).toContain("<em>it</em>");
    expect(out).toContain('class="md-inline"');
    expect(out).toContain('class="md-code"');
    expect(out).toContain("const a = 1;");
  });

  it("linkifies http(s) and keeps rel=noreferrer", () => {
    const out = html("see https://example.com/x");
    expect(out).toContain('href="https://example.com/x"');
    expect(out).toContain('rel="noopener noreferrer"');
  });

  it("never interprets raw HTML", () => {
    const out = html('<img src=x onerror=alert(1)>');
    expect(out).not.toContain("<img");
    expect(out).toContain("&lt;img");
  });

  it("highlights @mentions of room members", () => {
    const out = html("@Alice hi", [member("Alice")]);
    expect(out).toContain('class="mention"');
  });

  it("leaves unknown @names as text", () => {
    const out = html("@Nobody hi", [member("Alice")]);
    expect(out).not.toContain('class="mention"');
  });
});
