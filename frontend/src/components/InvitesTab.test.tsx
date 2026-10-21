import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Invite } from "../api";
import { InvitesTab, InvitesView } from "./InvitesTab";

const apiMock = vi.hoisted(() => vi.fn());
vi.mock("../api", () => ({ api: apiMock }));

function invite(id: string, overrides: Partial<Invite> = {}): Invite {
  return {
    invite_id: id,
    kind: "registration",
    max_role: "user",
    created_at: "2026-10-21T00:00:00Z",
    expires_at: "2026-10-22T00:00:00Z",
    consumed_at: null,
    consumed_by: null,
    revoked_at: null,
    active: true,
    ...overrides,
  };
}

function renderView(invites: Invite[]) {
  const onRevoke = vi.fn();
  const result = render(
    <InvitesView
      invites={invites}
      created={null}
      copied={null}
      error={null}
      onCreate={vi.fn()}
      onRevoke={onRevoke}
      onCopyLink={vi.fn()}
      onCopyToken={vi.fn()}
    />,
  );
  return { ...result, onRevoke };
}

describe("InvitesView", () => {
  it("only offers revoke on active invites and preserves row order", async () => {
    const active = invite("active-id");
    const used = invite("used-id", {
      active: false,
      consumed_at: "2026-10-21T01:00:00Z",
      consumed_by: "u2",
    });
    const expired = invite("expired-id", { active: false });
    const { container, onRevoke } = renderView([active, used, expired]);
    const rows = container.querySelectorAll("tbody tr");

    expect(rows).toHaveLength(3);
    expect(within(rows[0] as HTMLElement).getByRole("button", { name: "撤销" })).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).queryByRole("button", { name: "撤销" })).not.toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByText("已使用")).toHaveClass("tag", "warn");
    expect(within(rows[2] as HTMLElement).queryByRole("button", { name: "撤销" })).not.toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).getByText("已过期")).toHaveClass("tag", "warn");

    const user = userEvent.setup();
    await user.click(within(rows[0] as HTMLElement).getByRole("button", { name: "撤销" }));
    expect(onRevoke).toHaveBeenCalledWith("active-id");
  });

  it("renders an empty table without a placeholder or revoke action", () => {
    const { container } = renderView([]);

    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(container.querySelectorAll("tbody tr")).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "撤销" })).not.toBeInTheDocument();
  });
});

describe("InvitesTab", () => {
  beforeEach(() => {
    apiMock.mockReset();
  });

  it("shows the 409 conflict message when revoke is rejected", async () => {
    const active = invite("active-id");
    apiMock.mockImplementation((path: string) => {
      if (path === "/api/v1/admin/invites") return Promise.resolve({ invites: [active] });
      if (path.endsWith("/revoke")) {
        return Promise.reject(
          Object.assign(new Error("邀请已被使用，无法撤销"), { status: 409 }),
        );
      }
      return Promise.resolve({});
    });

    render(<InvitesTab />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "撤销" }));

    expect(await screen.findByText("邀请已被使用，无法撤销")).toHaveClass("error");
  });
});
