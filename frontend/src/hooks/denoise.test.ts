import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_DEEPFILTERNET3_PARAMS,
  DEFAULT_DTLN_PARAMS,
  getNoiseSettings,
  isNoiseMode,
  isWasmNoiseMode,
  noiseParamsKey,
  setNoisePrefs,
  subscribeNoise,
} from "./denoise";

describe("noise settings", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("keeps the existing default while supplying default advanced params", () => {
    const settings = getNoiseSettings();
    expect(settings.mode).toBe("webrtc");
    expect(settings.aec).toBe(true);
    expect(settings.agc).toBe(true);
    expect(settings.params.deepfilternet3).toEqual(DEFAULT_DEEPFILTERNET3_PARAMS);
    expect(settings.params.dtln).toEqual(DEFAULT_DTLN_PARAMS);
  });

  it("persists and merges model-specific params", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeNoise(listener);
    setNoisePrefs({
      mode: "deepfilternet3",
      params: {
        deepfilternet3: { ...DEFAULT_DEEPFILTERNET3_PARAMS, minSpeechFrames: 2 },
        dtln: { ...DEFAULT_DTLN_PARAMS },
      },
    });

    const settings = getNoiseSettings();
    expect(settings.mode).toBe("deepfilternet3");
    expect(settings.params.deepfilternet3.minSpeechFrames).toBe(2);
    expect(noiseParamsKey(settings)).toContain("minSpeechFrames");
    expect(listener).toHaveBeenCalledTimes(1);
    unsubscribe();
  });

  it("rejects unknown persisted modes and validates provider guards", () => {
    localStorage.setItem(
      "cloudroom.noise",
      JSON.stringify({ mode: "unknown", aec: false, agc: true }),
    );
    expect(getNoiseSettings().mode).toBe("webrtc");
    expect(isNoiseMode("gtcrn")).toBe(true);
    expect(isNoiseMode("bogus")).toBe(false);
    expect(isWasmNoiseMode("deepfilternet3")).toBe(true);
    expect(isWasmNoiseMode("webrtc")).toBe(false);
  });
});
