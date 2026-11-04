import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { User } from "../api";
import { UsersView } from "./UsersTab";

function user(id: string, role: User["role"], disabled = false): User {
  return {
    user_id: id,
    username: id,
    role,
    avatar_url: null,
    created_at: "2026-10-21T00:00:00Z",
    disabled,
  };
}

const admin = user("admin-id", "admin");
const normal = user("user-id", "user", true);
const superAdmin = user("super-id", "super_admin");

function renderView(users: User[], isSuper = true) {
  const onResetPassword = vi.fn();
  const onToggleDisabled = vi.fn();
  const onRoleChange = vi.fn();
  const result = render(
    <UsersView
      users={users}
      isSuper={isSuper}
      error={null}
      temporaryPassword={null}
      onResetPassword={onResetPassword}
      onToggleDisabled={onToggleDisabled}
      onRoleChange={onRoleChange}
    />,
  );
  return { ...result, onResetPassword, onToggleDisabled, onRoleChange };
}

describe("UsersView", () => {
  it("preserves role-select and account-action visibility", async () => {
    const { container, onResetPassword, onToggleDisabled, onRoleChange } = renderView([
      admin,
      normal,
      superAdmin,
    ]);
    const rows = container.querySelectorAll("tbody tr");
    const userEventInstance = userEvent.setup();

    expect(rows).toHaveLength(3);
    expect(screen.getAllByRole("button", { name: "重置密码" })).toHaveLength(3);
    expect(within(rows[0] as HTMLElement).getByRole("button", { name: "禁用" })).toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByRole("button", { name: "启用" })).toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).queryByRole("button", { name: "禁用" })).not.toBeInTheDocument();
    expect(within(rows[2] as HTMLElement).queryByRole("button", { name: "启用" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("combobox")).toHaveLength(2);
    expect(within(rows[2] as HTMLElement).queryByRole("combobox")).not.toBeInTheDocument();

    await userEventInstance.click(within(rows[0] as HTMLElement).getByRole("button", { name: "重置密码" }));
    expect(onResetPassword).toHaveBeenCalledWith("admin-id");
    await userEventInstance.click(within(rows[0] as HTMLElement).getByRole("button", { name: "禁用" }));
    expect(onToggleDisabled).toHaveBeenCalledWith("admin-id", true);
    await userEventInstance.selectOptions(
      within(rows[0] as HTMLElement).getByRole("combobox"),
      "user",
    );
    expect(onRoleChange).toHaveBeenCalledWith("admin-id", "user");
  });

  it("hides role selects from non-super admins", () => {
    const { container } = renderView([admin, normal], false);
    const rows = container.querySelectorAll("tbody tr");

    expect(screen.queryAllByRole("combobox")).toHaveLength(0);
    expect(screen.queryAllByRole("button", { name: "重置密码" })).toHaveLength(0);
    expect(within(rows[0] as HTMLElement).queryByRole("button", { name: "禁用" })).not.toBeInTheDocument();
    expect(within(rows[1] as HTMLElement).getByRole("button", { name: "启用" })).toBeInTheDocument();
  });

  it("renders error, temporary password, and an empty table", () => {
    const { container } = render(
      <UsersView
        users={[]}
        isSuper={false}
        error="加载失败"
        temporaryPassword="temp-pw"
        onResetPassword={vi.fn()}
        onToggleDisabled={vi.fn()}
        onRoleChange={vi.fn()}
      />,
    );

    expect(screen.getByText("加载失败")).toHaveClass("error");
    expect(screen.getByText("temp-pw")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "用户名" })).toBeInTheDocument();
    expect(container.querySelectorAll("tbody tr")).toHaveLength(0);
  });
});
