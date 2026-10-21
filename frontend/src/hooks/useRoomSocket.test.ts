import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useRoomSocket } from "./useRoomSocket";

class FakeWebSocket {
  static OPEN = 1;
  static instances: FakeWebSocket[] = [];

  readyState = 0;
  url: string;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  send = vi.fn();
  close = vi.fn(() => {
    this.readyState = 3;
    this.onclose?.(new Event("close") as CloseEvent);
  });

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  open() {
    this.readyState = FakeWebSocket.OPEN;
    this.onopen?.(new Event("open"));
  }

  message(payload: unknown) {
    this.onmessage?.({ data: JSON.stringify(payload) } as MessageEvent);
  }

  serverClose() {
    this.readyState = 3;
    this.onclose?.(new Event("close") as CloseEvent);
  }
}

describe("useRoomSocket", () => {
  beforeEach(() => {
    FakeWebSocket.instances = [];
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", FakeWebSocket);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("connects, forwards events and reconnects after a server close", () => {
    const onEvent = vi.fn();
    const { result } = renderHook(() => useRoomSocket("abc1234", "u1", onEvent));

    expect(FakeWebSocket.instances).toHaveLength(1);
    const first = FakeWebSocket.instances[0];
    act(() => first.open());
    expect(result.current.connected).toBe(true);

    act(() => first.message({ type: "hello", voice_members: ["u1"] }));
    expect(result.current.voiceMembers).toEqual(["u1"]);
    expect(onEvent).toHaveBeenCalledWith(expect.objectContaining({ type: "hello" }));

    act(() => first.message({ type: "voice_state", members: ["u1", "u2"] }));
    expect(result.current.voiceMembers).toEqual(["u1", "u2"]);

    act(() => first.serverClose());
    expect(result.current.connected).toBe(false);
    act(() => vi.advanceTimersByTime(1000));
    expect(FakeWebSocket.instances).toHaveLength(2);
  });

  it("closes the socket and stops reconnecting on unmount", () => {
    const { result, unmount } = renderHook(() => useRoomSocket("abc1234", "u1", vi.fn()));
    const ws = FakeWebSocket.instances[0];
    act(() => ws.open());
    expect(result.current.connected).toBe(true);

    act(() => unmount());
    expect(ws.close).toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(60_000));
    expect(FakeWebSocket.instances).toHaveLength(1);
  });
});
