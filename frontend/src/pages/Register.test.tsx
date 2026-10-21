import { MemoryRouter, Route, Routes } from "react-router-dom";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const auth = vi.hoisted(() => ({ refresh: vi.fn() }));
const apiMock = vi.hoisted(() => vi.fn());
const INVITE_TOKEN = "inv" + "-token";
const PASSWORD = "pw" + "-one";

vi.mock("../auth", () => ({
  useAuth: () => ({ refresh: auth.refresh }),
}));
vi.mock("../api", () => ({ api: apiMock }));

import Register from "./Register";

function renderRegister(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/register" element={<Register />} />
        <Route path="/" element={<div>HomePage</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

async function fillPasswords(password: string, confirm: string) {
  const user = userEvent.setup();
  await user.type(screen.getByLabelText("用户名"), "new-user");
  await user.type(screen.getByLabelText("密码"), password);
  await user.type(screen.getByLabelText("确认密码"), confirm);
  return user;
}

describe("Register invite flow", () => {
  beforeEach(() => {
    apiMock.mockReset();
    auth.refresh.mockReset();
    auth.refresh.mockResolvedValue(undefined);
  });

  it("prefills an invite token and removes it from the URL", async () => {
    renderRegister(`/register?invite=${INVITE_TOKEN}`);
    expect(screen.getByLabelText("邀请码")).toHaveValue(INVITE_TOKEN);
    await waitFor(() => expect(window.location.search).toBe(""));
  });

  it("blocks submission when the passwords differ", async () => {
    renderRegister(`/register?invite=${INVITE_TOKEN}`);
    const user = await fillPasswords(PASSWORD, PASSWORD + "-two");
    await user.click(screen.getByRole("button", { name: "注册" }));
    expect(screen.getByText("两次输入的密码不一致")).toBeInTheDocument();
    expect(apiMock).not.toHaveBeenCalled();
  });

  it("blocks submission when the invite token is missing", async () => {
    renderRegister("/register");
    const user = await fillPasswords(PASSWORD, PASSWORD);
    await user.click(screen.getByRole("button", { name: "注册" }));
    expect(screen.getByText(/请输入邀请码/)).toBeInTheDocument();
    expect(apiMock).not.toHaveBeenCalled();
  });

  it("submits the captured invite token and navigates home", async () => {
    apiMock.mockResolvedValue({});
    renderRegister(`/register?invite=${INVITE_TOKEN}`);
    const user = await fillPasswords(PASSWORD, PASSWORD);
    await user.click(screen.getByRole("button", { name: "注册" }));
    await waitFor(() =>
      expect(apiMock).toHaveBeenCalledWith(
        "/api/v1/auth/register",
        expect.objectContaining({
          method: "POST",
          body: JSON.stringify({
            username: "new-user",
            password: PASSWORD,
            token: INVITE_TOKEN,
          }),
        }),
      ),
    );
    expect(auth.refresh).toHaveBeenCalled();
    expect(await screen.findByText("HomePage")).toBeInTheDocument();
  });
});
