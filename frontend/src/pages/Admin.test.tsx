import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const auth = vi.hoisted(() => ({
  me: {
    user: {
      user_id: "admin-id",
      username: "admin",
      role: "admin",
      avatar_url: null,
      created_at: "2026-10-21T00:00:00Z",
      disabled: false,
    },
    csrf_token: "csrf",
    session_expires_at: "2026-11-21T00:00:00Z",
  },
}));
const apiMock = vi.hoisted(() => vi.fn());

vi.mock("../auth", () => ({
  useAuth: () => auth,
}));
vi.mock("../api", () => ({ api: apiMock }));

import Admin from "./Admin";

beforeEach(() => {
  apiMock.mockReset();
  apiMock.mockImplementation((path: string) => {
    if (path === "/api/v1/admin/stats") {
      return Promise.resolve({
        users: 0,
        rooms: 0,
        rooms_active: 0,
        rooms_deleted: 0,
        rooms_expired: 0,
        room_requests_pending: 0,
        messages: 0,
      });
    }
    if (path === "/api/v1/admin/invites") return Promise.resolve({ invites: [] });
    if (path === "/api/v1/admin/users") return Promise.resolve({ users: [] });
    return Promise.resolve({});
  });
});

describe("Admin tabs", () => {
  it("defaults to the overview tab", () => {
    render(<Admin />);

    expect(screen.getByRole("heading", { name: "管理后台" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "总览" })).toHaveClass("active");
    expect(screen.queryByRole("button", { name: "生成注册邀请" })).not.toBeInTheDocument();
  });

  it("switches between invites and users tabs", async () => {
    const user = userEvent.setup();
    render(<Admin />);

    await user.click(screen.getByRole("button", { name: "邀请" }));
    expect(await screen.findByRole("button", { name: "生成注册邀请" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "邀请" })).toHaveClass("active");
    expect(screen.getByRole("button", { name: "总览" })).not.toHaveClass("active");

    await user.click(screen.getByRole("button", { name: "用户" }));
    expect(await screen.findByRole("columnheader", { name: "用户名" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "生成注册邀请" })).not.toBeInTheDocument();
  });
});
