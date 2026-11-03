import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const auth = vi.hoisted(() => ({
  refresh: vi.fn(),
  me: {
    user: {
      user_id: "u1",
      username: "tester",
      role: "user",
      avatar_url: null,
      created_at: "2026-10-21T00:00:00Z",
      disabled: false,
    },
    csrf_token: "csrf",
    session_expires_at: "2026-11-21T00:00:00Z",
  },
}));
const apiMock = vi.hoisted(() => vi.fn());
const OLD_PASSWORD = "old" + "-pw";
const NEW_PASSWORD = "new" + "-pw";
const denoise = vi.hoisted(() => ({
  getNoiseSettings: vi.fn(() => ({ mode: "webrtc", aec: true, agc: true })),
  setNoisePrefs: vi.fn(),
}));

vi.mock("../auth", () => ({
  useAuth: () => ({ me: auth.me, refresh: auth.refresh }),
}));
vi.mock("../api", () => ({ api: apiMock, invalidateMe: vi.fn() }));
vi.mock("../hooks/denoise", () => denoise);

import SettingsModal from "./Settings";

async function fillPasswordForm(current: string, next: string, confirm: string) {
  const user = userEvent.setup();
  await user.type(screen.getByPlaceholderText("当前密码"), current);
  await user.type(screen.getByPlaceholderText("新密码"), next);
  await user.type(screen.getByPlaceholderText("确认新密码"), confirm);
  return user;
}

describe("Settings password form", () => {
  beforeEach(() => {
    apiMock.mockReset();
    auth.refresh.mockReset();
    auth.refresh.mockResolvedValue(undefined);
  });

  it("rejects mismatched new passwords before calling the API", async () => {
    render(<SettingsModal onClose={() => {}} />);
    const user = await fillPasswordForm(OLD_PASSWORD, NEW_PASSWORD, NEW_PASSWORD + "-x");
    await user.click(screen.getByRole("button", { name: "修改密码" }));
    expect(screen.getByText("两次输入的新密码不一致")).toBeInTheDocument();
    expect(
      apiMock.mock.calls.filter(([u]) => String(u).includes("/me/password")),
    ).toHaveLength(0);
  });

  it("submits a valid password change and clears the fields", async () => {
    apiMock.mockResolvedValue({ ok: true });
    render(<SettingsModal onClose={() => {}} />);
    const user = await fillPasswordForm(OLD_PASSWORD, NEW_PASSWORD, NEW_PASSWORD);
    await user.click(screen.getByRole("button", { name: "修改密码" }));

    await waitFor(() =>
      expect(apiMock).toHaveBeenCalledWith(
        "/api/v1/me/password",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({
            current_password: OLD_PASSWORD,
            new_password: NEW_PASSWORD,
          }),
        }),
      ),
    );
    expect(await screen.findByText("密码已修改。")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("当前密码")).toHaveValue("");
    expect(screen.getByPlaceholderText("新密码")).toHaveValue("");
    expect(screen.getByPlaceholderText("确认新密码")).toHaveValue("");
  });

  it("shows an API error and keeps the form usable", async () => {
    apiMock.mockRejectedValue(new Error("当前密码不正确"));
    render(<SettingsModal onClose={() => {}} />);
    const user = await fillPasswordForm("wrong", NEW_PASSWORD, NEW_PASSWORD);
    await user.click(screen.getByRole("button", { name: "修改密码" }));

    expect(await screen.findByText("当前密码不正确")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("当前密码")).toHaveValue("wrong");
    expect(screen.getByRole("button", { name: "修改密码" })).not.toBeDisabled();
  });
});
