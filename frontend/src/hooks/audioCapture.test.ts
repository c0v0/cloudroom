import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import {
  buildMicConstraints,
  ensureEchoCancellation,
  openMicrophone,
} from "./audioCapture";
import type { AecStatus } from "./audioCapture";

type ApplyConstraints = Mock<(constraints: MediaTrackConstraints) => Promise<void>>;

interface FakeTrack {
  track: {
    kind: "audio";
    getSettings: () => MediaTrackSettings;
    applyConstraints: ApplyConstraints;
  };
  applyConstraints: ApplyConstraints;
}

function fakeTrack(initial: MediaTrackSettings, applied?: MediaTrackSettings): FakeTrack {
  let settings = { ...initial };
  const applyConstraints: ApplyConstraints = vi.fn(async (constraints: MediaTrackConstraints) => {
    if (applied) settings = { ...settings, ...applied };
    expect(constraints).toEqual({ echoCancellation: true });
  });
  const track = {
    kind: "audio" as const,
    getSettings: vi.fn(() => ({ ...settings })),
    applyConstraints,
  };
  return { track, applyConstraints };
}

function fakeStream(track: FakeTrack["track"] | null) {
  return {
    getAudioTracks: () => (track ? [track] : []),
  } as unknown as MediaStream;
}

describe("buildMicConstraints", () => {
  it("maps AEC, browser noise suppression and AGC preferences", () => {
    expect(buildMicConstraints({ aec: true, mode: "webrtc", agc: true })).toEqual({
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    });
    expect(buildMicConstraints({ aec: true, mode: "rnnoise", agc: false })).toEqual({
      echoCancellation: true,
      noiseSuppression: false,
      autoGainControl: false,
    });
    expect(buildMicConstraints({ aec: false, mode: "off", agc: false })).toEqual({
      echoCancellation: false,
      noiseSuppression: false,
      autoGainControl: false,
    });
  });
});

describe("ensureEchoCancellation", () => {
  it("reports applied without calling applyConstraints when the browser already honored AEC", async () => {
    const { track, applyConstraints } = fakeTrack({ echoCancellation: true });
    await expect(ensureEchoCancellation(fakeStream(track), true)).resolves.toBe("applied");
    expect(applyConstraints).not.toHaveBeenCalled();
  });

  it("attempts applyConstraints when the browser ignored the initial AEC request", async () => {
    const { track, applyConstraints } = fakeTrack(
      { echoCancellation: false },
      { echoCancellation: true },
    );
    await expect(ensureEchoCancellation(fakeStream(track), true)).resolves.toBe("applied");
    expect(applyConstraints).toHaveBeenCalledWith({ echoCancellation: true });
  });

  it("reports unsupported if applyConstraints leaves AEC disabled or fails", async () => {
    const disabled = fakeTrack({ echoCancellation: false });
    await expect(ensureEchoCancellation(fakeStream(disabled.track), true)).resolves.toBe(
      "unsupported",
    );

    const rejected = fakeTrack({ echoCancellation: false });
    rejected.applyConstraints.mockRejectedValueOnce(new Error("not supported"));
    await expect(ensureEchoCancellation(fakeStream(rejected.track), true)).resolves.toBe(
      "unsupported",
    );
  });

  it("respects the user's off switch and handles missing tracks", async () => {
    const { track, applyConstraints } = fakeTrack({ echoCancellation: true });
    await expect(ensureEchoCancellation(fakeStream(track), false)).resolves.toBe("off");
    expect(applyConstraints).not.toHaveBeenCalled();

    await expect(ensureEchoCancellation(fakeStream(null), true)).resolves.toBe("unknown");
  });
});

describe("openMicrophone", () => {
  beforeEach(() => {
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: vi.fn() },
    });
  });

  it("opens the requested stream and returns the actual AEC status", async () => {
    const { track } = fakeTrack({ echoCancellation: true });
    const stream = fakeStream(track);
    vi.mocked(navigator.mediaDevices.getUserMedia).mockResolvedValueOnce(stream);

    const result = await openMicrophone({ aec: true, mode: "webrtc", agc: true });

    expect(navigator.mediaDevices.getUserMedia).toHaveBeenCalledWith({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
    });
    expect(result.stream).toBe(stream);
    expect(result.aecStatus).toBe<AecStatus>("applied");
  });
});
