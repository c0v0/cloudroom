import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { RoomSocket } from "./useRoomSocket";

const adapters = vi.hoisted(() => {
  const p2pJoin = vi.fn();
  const lkJoin = vi.fn();
  const p2pSignal = vi.fn();
  return {
    p2pArgs: null as { enabled?: boolean } | null,
    lkArgs: null as { enabled?: boolean } | null,
    p2pJoin,
    lkJoin,
    p2pSignal,
    p2pState: {
      status: "connected",
      inVoice: true,
      micMuted: false,
      speakerMuted: true,
      noiseMode: "webrtc",
      aec: true,
      aecStatus: "applied",
      agc: true,
      join: p2pJoin,
      leave: vi.fn(),
      toggleMicrophone: vi.fn(),
      toggleSpeaker: vi.fn(),
      applyMicMuted: vi.fn(),
      setNoiseMode: vi.fn(),
      setAec: vi.fn(),
      setAgc: vi.fn(),
      setRemoteVolume: vi.fn(),
      quality: "good",
      handleSignal: p2pSignal,
    },
    lkState: {
      status: "reconnecting",
      inVoice: false,
      participantIds: ["u1", "u2", "u3"],
      micMuted: true,
      speakerMuted: false,
      noiseMode: "off",
      aec: false,
      aecStatus: "unsupported",
      agc: false,
      join: lkJoin,
      leave: vi.fn(),
      toggleMicrophone: vi.fn(),
      toggleSpeaker: vi.fn(),
      applyMicMuted: vi.fn(),
      setNoiseMode: vi.fn(),
      setAec: vi.fn(),
      setAgc: vi.fn(),
      setRemoteVolume: vi.fn(),
      quality: "poor",
      audioContainerRef: { current: null },
    },
  };
});

vi.mock("./useP2PVoice", () => ({
  useP2PVoice: (args: { enabled?: boolean }) => {
    adapters.p2pArgs = args;
    return adapters.p2pState;
  },
}));
vi.mock("./useLiveKitVoice", () => ({
  useLiveKitVoice: (args: { enabled?: boolean }) => {
    adapters.lkArgs = args;
    return adapters.lkState;
  },
}));

import { useVoiceSession } from "./useVoiceSession";

const socket = {
  connected: true,
  voiceMembers: ["u1", "u2"],
  send: vi.fn(),
} as unknown as RoomSocket;

describe("useVoiceSession mode selection", () => {
  beforeEach(() => {
    adapters.p2pArgs = null;
    adapters.lkArgs = null;
    adapters.p2pJoin.mockClear();
    adapters.lkJoin.mockClear();
    adapters.p2pSignal.mockClear();
  });

  it("selects the p2p adapter for a p2p room", () => {
    const { result } = renderHook(() =>
      useVoiceSession({ code: "room", mode: "p2p", myUserId: "u1", socket }),
    );

    expect(adapters.p2pArgs?.enabled).toBe(true);
    expect(adapters.lkArgs?.enabled).toBe(false);
    expect(result.current.mode).toBe("p2p");
    expect(result.current.status).toBe("connected");
    expect(result.current.inVoice).toBe(true);
    expect(result.current.participantCount).toBe(2);
    expect(result.current.aecStatus).toBe("applied");
    expect(result.current.join).toBe(adapters.p2pJoin);
    expect(result.current.handleSignal).toBe(adapters.p2pSignal);
  });

  it("switches selection to sfu without auto-joining either adapter", () => {
    const { result, rerender } = renderHook(
      ({ mode }: { mode: "p2p" | "sfu" }) =>
        useVoiceSession({ code: "room", mode, myUserId: "u1", socket }),
      { initialProps: { mode: "p2p" } },
    );

    rerender({ mode: "sfu" });

    expect(adapters.p2pArgs?.enabled).toBe(false);
    expect(adapters.lkArgs?.enabled).toBe(true);
    expect(result.current.mode).toBe("sfu");
    expect(result.current.status).toBe("reconnecting");
    expect(result.current.inVoice).toBe(false);
    expect(result.current.participantCount).toBe(3);
    expect(result.current.aecStatus).toBe("unsupported");
    expect(result.current.join).toBe(adapters.lkJoin);
    expect(result.current.micMuted).toBe(true);
    expect(adapters.p2pJoin).not.toHaveBeenCalled();
    expect(adapters.lkJoin).not.toHaveBeenCalled();
  });
});
