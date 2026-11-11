import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { Member, Message } from "../api";
import { mergeMessages } from "../lib/messages";
import { fmtMsgTime } from "../lib/time";
import { AttachmentView, MessageList } from "./MessageList";

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
        onEdit={async () => true}
        onDelete={() => undefined}
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

  it("treats a null user_id sender object as a system message", () => {
    const system = userMessage("sys-null-id", 2, "系统消息", {
      sender: { user_id: null, username: null, nickname: null },
    });
    const { container } = renderList([system]);
    const item = screen.getByRole("listitem");
    expect(item).toHaveClass("message", "system");
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

  it("marks the room owner by user_id even when nicknames collide", () => {
    const ownerMembers: Member[] = [
      {
        ...members[0],
        user_id: "owner-id",
        username: "boss",
        nickname: "Alice",
        room_role: "owner",
      },
      { ...members[0], user_id: "u1", username: "alice", nickname: "Alice" },
    ];
    render(
      <ul className="messages">
        <MessageList
          messages={[
            userMessage("m1", 1, "from owner", {
              sender: { user_id: "owner-id", username: "boss", nickname: "Alice" },
            }),
            userMessage("m2", 2, "from member"),
          ]}
          members={ownerMembers}
          onReply={vi.fn()}
          onCopy={vi.fn()}
          onReact={vi.fn()}
          onEdit={async () => true}
          onDelete={() => undefined}
        />
      </ul>,
    );
    const badges = document.querySelectorAll(".msg-owner");
    expect(badges).toHaveLength(1);
    expect(badges[0].closest("li")).toHaveAttribute("id", "msg-m1");
  });
});

describe("AttachmentView", () => {
  const attachment = {
    id: "a1",
    filename: "shot.png",
    mime: "image/png",
    size: 123,
    url: "/api/v1/rooms/r1/attachments/a1",
    thumbnail_url: "/api/v1/rooms/r1/attachments/a1/thumbnail",
  };

  it("uses the thumbnail and falls back to the original on error", () => {
    render(<AttachmentView attachment={attachment} />);
    const img = screen.getByRole("img") as HTMLImageElement;
    expect(img.getAttribute("src")).toBe(attachment.thumbnail_url);
    fireEvent.error(img);
    expect(img.getAttribute("src")).toBe(attachment.url);
    fireEvent.error(img);
    expect(img.getAttribute("src")).toBe(attachment.url);
  });
});

describe("AttachmentView media", () => {
  const base = {
    id: "a2",
    filename: "clip",
    size: 100,
    url: "/api/v1/rooms/r1/attachments/a2",
    thumbnail_url: null,
  };

  it("renders audio and video players", () => {
    const audio = render(<AttachmentView attachment={{ ...base, mime: "audio/mpeg" }} />);
    expect(audio.container.querySelector("audio")).toHaveAttribute(
      "src",
      base.url,
    );
    audio.unmount();
    const video = render(<AttachmentView attachment={{ ...base, mime: "video/mp4" }} />);
    expect(video.container.querySelector("video")).toHaveAttribute(
      "src",
      base.url,
    );
  });
});

describe("AttachmentView PDF", () => {
  it("loads the file as a blob only when preview is opened", async () => {
    const createObjectURL = vi.fn(() => "blob:pdf-preview");
    const revokeObjectURL = vi.fn();
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL,
    });
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      blob: async () => new Blob(["%PDF"], { type: "application/pdf" }),
    });
    vi.stubGlobal("fetch", fetchMock);

    const attachment = {
      id: "pdf1",
      filename: "doc.pdf",
      mime: "application/pdf",
      size: 1024,
      url: "/api/v1/rooms/r1/attachments/pdf1",
      thumbnail_url: null,
    };
    const { unmount } = render(<AttachmentView attachment={attachment} />);
    expect(screen.queryByTitle("doc.pdf")).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "下载" })).toHaveAttribute(
      "href",
      `${attachment.url}?download=1`,
    );

    await userEvent.click(screen.getByRole("button", { name: "预览 PDF" }));
    const frame = await screen.findByTitle("doc.pdf");
    expect(frame).toHaveAttribute("src", "blob:pdf-preview");
    expect(fetchMock).toHaveBeenCalledWith(attachment.url, {
      credentials: "include",
    });

    unmount();
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:pdf-preview");
    vi.unstubAllGlobals();
    delete (URL as unknown as { createObjectURL?: unknown }).createObjectURL;
    delete (URL as unknown as { revokeObjectURL?: unknown }).revokeObjectURL;
  });
});

describe("MessageList bookmarks", () => {
  it("shows bookmark state and invokes the toggle", () => {
    const onBookmark = vi.fn();
    const message = userMessage("b1", 1, "save me");
    const { rerender } = render(
      <ul className="messages">
        <MessageList
          messages={[message]}
          members={members}
          onReply={vi.fn()}
          onCopy={vi.fn()}
          onReact={vi.fn()}
          onEdit={async () => true}
          onDelete={vi.fn()}
          onBookmark={onBookmark}
          bookmarkedIds={new Set()}
        />
      </ul>,
    );
    fireEvent.click(screen.getByRole("button", { name: "收藏" }));
    expect(onBookmark).toHaveBeenCalledWith(message);
    rerender(
      <ul className="messages">
        <MessageList
          messages={[message]}
          members={members}
          onReply={vi.fn()}
          onCopy={vi.fn()}
          onReact={vi.fn()}
          onEdit={async () => true}
          onDelete={vi.fn()}
          onBookmark={onBookmark}
          bookmarkedIds={new Set([message.id])}
        />
      </ul>,
    );
    expect(screen.getByRole("button", { name: "已收藏" })).toBeInTheDocument();
  });
});
