import { MemoryRouter } from "react-router-dom";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const apiMock = vi.hoisted(() => vi.fn());
const refreshMock = vi.hoisted(() => vi.fn());

vi.mock("../api", () => ({ api: apiMock }));
vi.mock("../auth", () => ({ useAuth: () => ({ refresh: refreshMock }) }));

import Login from "./Login";

describe("Login password handling", () => {
  beforeEach(() => {
    apiMock.mockReset();
    refreshMock.mockReset();
  });

  it("clears and refocuses the password after invalid credentials", async () => {
    apiMock.mockRejectedValue(
      Object.assign(new Error("用户名或密码错误"), { status: 401, code: "unauthorized" }),
    );
    const user = userEvent.setup();
    render(
      <MemoryRouter>
        <Login />
      </MemoryRouter>,
    );

    await user.type(screen.getByLabelText("用户名"), "alice");
    await user.type(screen.getByLabelText("密码"), "wrong-password");
    await user.click(screen.getByRole("button", { name: "登录" }));

    expect(await screen.findByText("用户名或密码错误")).toBeInTheDocument();
    const password = screen.getByLabelText("密码");
    expect(password).toHaveValue("");
    expect(password).toHaveFocus();
    expect(screen.getByLabelText("用户名")).toHaveValue("alice");
    expect(refreshMock).not.toHaveBeenCalled();
  });
});
