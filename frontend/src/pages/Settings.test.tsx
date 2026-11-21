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
  getNoiseSettings: vi.fn(() => ({
    mode: "webrtc",
    aec: true,
    agc: true,
    params: {
      deepfilternet3: {
        speechAttenuationDb: 25,
        pauseAttenuationDb: 45,
        pauseGateLookaheadFrames: 3,
        minSpeechFrames: 1,
        postGain: false,
        maxLoad: 0.7,
        bypassUntilReady: true,
        readyTimeoutMs: 30000,
      },
      dtln: { bypassUntilReady: true, readyTimeoutMs: 30000 },
    },
  })),
  setNoisePrefs: vi.fn(),
  NOISE_PROVIDERS: [
    { id: "off", label: "关闭", summary: "", sampleRate: 0, latencyMs: 0, downloadBytes: 0, advanced: false },
    { id: "webrtc", label: "WebRTC", summary: "浏览器原生", sampleRate: 0, latencyMs: 0, downloadBytes: 0, advanced: false },
  ],
  noiseProvider: vi.fn(() => ({ summary: "浏览器原生", sampleRate: 0 })),
  noiseLabel: vi.fn((mode: string) => mode),
  noiseErrorMessage: vi.fn((error: unknown) => String(error)),
  isWasmNoiseMode: vi.fn((mode: string) => mode !== "off" && mode !== "webrtc"),
  benchmarkNoiseProvider: vi.fn(),
  measureNoiseLatency: vi.fn(),
  getNoiseRuntimeStatus: vi.fn(() => ({
    requested: "webrtc",
    actual: "webrtc",
    state: "idle",
    message: "",
  })),
  subscribeNoiseRuntime: vi.fn(() => () => undefined),
}));

vi.mock("../auth", () => ({
  useAuth: () => ({ me: auth.me, refresh: auth.refresh }),
}));
vi.mock("../api", () => ({ api: apiMock, invalidateMe: vi.fn() }));
vi.mock("../hooks/denoise", () => denoise);

import SettingsModal from "./Settings";

async function fillPasswordForm(current: string, next: string, confirm: string) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "账号与安全" }));
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

  it("shows the speaking threshold in dBFS and a 10 ms stop-delay step", () => {
    localStorage.setItem("cloudroom.gateDb", "-50");
    render(<SettingsModal onClose={() => {}} />);

    const vad = screen.getByRole("slider", { name: /动效阈值/ });
    expect(vad).toHaveAttribute("min", "-48");
    expect(vad).toHaveAttribute("max", "-24");
    expect(vad).toHaveAttribute("step", "1");
    expect(screen.getByText("动效阈值（-48 dBFS）")).toBeInTheDocument();

    const hold = screen.getByRole("slider", { name: /停止延迟/ });
    expect(hold).toHaveAttribute("step", "10");

    const compressor = screen.getByRole("slider", { name: /压缩阈值/ });
    expect(compressor).toHaveAttribute("min", "-60");
    expect(compressor).toHaveAttribute("max", "0");
    localStorage.removeItem("cloudroom.gateDb");
  });
});
