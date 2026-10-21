import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Member, Message } from "../api";
import { mergeMessages } from "../lib/messages";
import { fmtMsgTime } from "../lib/time";
import { MessageList } from "./MessageList";

const members: Member[] = [
  {
    user_id: "u1",
    username: "alice",
    avatar_url: null,
    nickname: "Alice",
    room_role: "member",
    joined_at: "2026-10-20T00:00:00Z",
    last_online_at: null,
  },
];

function userMessage(
  id: string,
  seq: number,
  body: string,
  overrides: Partial<Message> = {},
): Message {
  return {
    id,
    seq,
    sender: { user_id: "u1", username: "alice", nickname: "Alice" },
    body,
    created_at: "2026-10-21T09:05:00",
    ...overrides,
  };
}

function renderList(messages: Message[]) {
  const onReply = vi.fn();
  const onCopy = vi.fn();
  const onReact = vi.fn();
  const result = render(
    <ul className="messages">
      <MessageList
        messages={messages}
        members={members}
        onReply={onReply}
        onCopy={onCopy}
        onReact={onReact}
      />
    </ul>,
  );
  return { ...result, onReply, onCopy, onReact };
}

describe("MessageList", () => {
  it("renders messages in order and does not repeat merged duplicates", () => {
    const first = userMessage("a", 1, "first");
    const second = userMessage("b", 2, "second");
    const third = userMessage("c", 3, "third");
    const merged = mergeMessages([first], [first, second, third]);
    const { container } = renderList(merged);

    const items = container.querySelectorAll("li.message");
    expect(items).toHaveLength(3);
    expect(
      Array.from(items).map((item) => item.querySelector(".body")?.textContent),
    ).toEqual(["first", "second", "third"]);
    expect(screen.getAllByText("first")).toHaveLength(1);
  });

  it("renders message timestamps with the existing time formatter", () => {
    const now = new Date(2026, 9, 21, 12, 0, 0);
    vi.useFakeTimers();
    vi.setSystemTime(now);
    const todayIso = new Date(2026, 9, 21, 9, 5, 0).toISOString();
    const olderIso = new Date(2025, 0, 2, 3, 4, 0).toISOString();
    const { container } = renderList([
      userMessage("today", 1, "today", { created_at: todayIso }),
      userMessage("older", 2, "older", { created_at: olderIso }),
    ]);

    const times = container.querySelectorAll(".msg-time");
    expect(times).toHaveLength(2);
    expect(times[0]).toHaveTextContent(fmtMsgTime(todayIso, now));
    expect(times[1]).toHaveTextContent(fmtMsgTime(olderIso, now));
    expect(times[0].textContent).toMatch(/^\d{2}:\d{2}$/);
  });

  it("keeps an empty list when there are no messages", () => {
    const { container } = renderList([]);

    expect(container.querySelector("ul.messages")).toBeInTheDocument();
    expect(container.querySelectorAll("li.message")).toHaveLength(0);
  });

  it("renders system messages without message actions", () => {
    const system = userMessage("sys", 1, "系统消息", { sender: null });
    const { container } = renderList([system]);

    const item = screen.getByRole("listitem");
    expect(item).toHaveClass("message", "system");
    expect(within(item).getByText("系统")).toHaveClass("who", "system");
    expect(within(item).getByText("系统消息")).toHaveClass("body");
    expect(within(item).queryByRole("button")).not.toBeInTheDocument();
    expect(container.querySelector(".reply-quote")).not.toBeInTheDocument();
  });

  it("renders reply quotes and keeps normal message actions", () => {
    const message = userMessage("reply", 2, "answer", {
      reply: { id: "orig", sender: "Alice", uid: "u1", body: "original body" },
    });
    const { container } = renderList([message]);

    expect(container.querySelector(".reply-quote")).toHaveTextContent(
      "回复 Alice：original body",
    );
    expect(within(screen.getByRole("listitem")).getByText("answer")).toHaveClass("body");
    expect(screen.getByRole("button", { name: "回复" })).toBeInTheDocument();
  });

  it("renders long bodies, links, and mentions without truncation", () => {
    const longText = "长".repeat(400);
    const url = "https://example.com/path?q=1";
    const { container } = renderList([
      userMessage("long", 1, `${longText} ${url} @Alice`),
    ]);

    const body = container.querySelector(".message .body") as HTMLElement;
    expect(body).toHaveClass("body");
    expect(body.textContent).toContain(longText);
    expect(body.textContent).toContain(url);
    expect(within(body).getByRole("link", { name: url })).toHaveClass("msg-link");
    expect(container.querySelector(".mention")).toHaveTextContent("@Alice");
  });

  it("delegates reply, copy, and reaction actions", async () => {
    const message = userMessage("m1", 1, "hello", {
      reactions: [
        { emoji: "👍", count: 2, me: true },
        { emoji: "🎉", count: 1, me: false },
      ],
    });
    const { onReply, onCopy, onReact } = renderList([message]);
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "回复" }));
    expect(onReply).toHaveBeenCalledWith(message);

    await user.click(screen.getByRole("button", { name: "复制" }));
    expect(onCopy).toHaveBeenCalledWith("hello");

    const picker = document.querySelector(".reaction-picker") as HTMLElement;
    await user.click(within(picker).getByRole("button", { name: "👍" }));
    expect(onReact).toHaveBeenCalledWith("m1", "👍");

    const row = document.querySelector(".reaction-row") as HTMLElement;
    await user.click(within(row).getByRole("button", { name: /🎉/ }));
    expect(onReact).toHaveBeenCalledWith("m1", "🎉");
  });
});
