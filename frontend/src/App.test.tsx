import { MemoryRouter } from "react-router-dom";
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { MeResponse, Role } from "./api";

const auth = vi.hoisted(() => ({
  current: {
    me: null as MeResponse | null,
    loading: false,
    refresh: vi.fn(),
    logout: vi.fn(),
  },
}));

vi.mock("./auth", () => ({
  useAuth: () => auth.current,
}));
vi.mock("./pages/Home", () => ({ default: () => <div>HomePage</div> }));
vi.mock("./pages/Admin", () => ({ default: () => <div>AdminPage</div> }));
vi.mock("./pages/Room", () => ({ default: () => <div>RoomPage</div> }));
vi.mock("./pages/Settings", () => ({ default: () => <div>SettingsPage</div> }));

import App from "./App";

function me(role: Role): MeResponse {
  return {
    user: {
      user_id: "u1",
      username: "tester",
      role,
      avatar_url: null,
      created_at: "2026-10-21T00:00:00Z",
      disabled: false,
    },
    csrf_token: "csrf",
    session_expires_at: "2026-11-21T00:00:00Z",
  };
}

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}

describe("App role navigation", () => {
  beforeEach(() => {
    auth.current.me = null;
    auth.current.loading = false;
  });

  it("shows the loading state while auth is unresolved", () => {
    auth.current.loading = true;
    renderAt("/");
    expect(screen.getByText("加载中…")).toBeInTheDocument();
    expect(screen.queryByText("管理后台")).not.toBeInTheDocument();
  });

  it("does not expose admin navigation to a normal user", () => {
    auth.current.me = me("user");
    renderAt("/");
    expect(screen.getByText("HomePage")).toBeInTheDocument();
    expect(screen.queryByText("管理后台")).not.toBeInTheDocument();
  });

  it("shows admin navigation to an admin", () => {
    auth.current.me = me("admin");
    renderAt("/");
    expect(screen.getByText("管理后台")).toBeInTheDocument();
  });

  it("redirects a normal user away from /admin", async () => {
    auth.current.me = me("user");
    renderAt("/admin");
    expect(await screen.findByText("HomePage")).toBeInTheDocument();
    expect(screen.queryByText("AdminPage")).not.toBeInTheDocument();
  });

  it("renders /admin for an admin", async () => {
    auth.current.me = me("admin");
    renderAt("/admin");
    expect(await screen.findByText("AdminPage")).toBeInTheDocument();
  });
});
