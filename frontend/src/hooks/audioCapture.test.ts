import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Mock } from "vitest";

import {
  buildMicConstraints,
  ensureAutoGainControl,
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
    void constraints;
    if (applied) settings = { ...settings, ...applied };
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

  it("applies AGC and noise suppression together when AEC needs a constraint update", async () => {
    const { track, applyConstraints } = fakeTrack(
      { echoCancellation: false },
      { echoCancellation: true },
    );
    await ensureEchoCancellation(fakeStream(track), true, {
      autoGainControl: true,
      noiseSuppression: true,
    });
    expect(applyConstraints).toHaveBeenCalledWith({
      echoCancellation: true,
      autoGainControl: true,
      noiseSuppression: true,
    });
  });
});

describe("ensureAutoGainControl", () => {
  it("does not touch the track when AGC already matches", async () => {
    const { track, applyConstraints } = fakeTrack({ autoGainControl: true });
    await ensureAutoGainControl(fakeStream(track), true);
    expect(applyConstraints).not.toHaveBeenCalled();
  });

  it("re-applies AGC when the browser ignored the initial request", async () => {
    const { track, applyConstraints } = fakeTrack(
      { autoGainControl: false },
      { autoGainControl: true },
    );
    await ensureAutoGainControl(fakeStream(track), true);
    expect(applyConstraints).toHaveBeenCalledWith({ autoGainControl: true });
  });

  it("turns AGC off when requested and tolerates missing tracks", async () => {
    const { track, applyConstraints } = fakeTrack(
      { autoGainControl: true },
      { autoGainControl: false },
    );
    await ensureAutoGainControl(fakeStream(track), false);
    expect(applyConstraints).toHaveBeenCalledWith({ autoGainControl: false });
    await expect(ensureAutoGainControl(fakeStream(null), true)).resolves.toBeUndefined();
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
    const { track, applyConstraints } = fakeTrack({ echoCancellation: true });
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
    expect(applyConstraints).toHaveBeenCalledWith({
      autoGainControl: true,
      echoCancellation: true,
      noiseSuppression: true,
    });
    expect(result.stream).toBe(stream);
    expect(result.aecStatus).toBe<AecStatus>("applied");
  });
});
