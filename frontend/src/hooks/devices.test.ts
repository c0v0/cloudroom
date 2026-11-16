import { beforeEach, describe, expect, it } from "vitest";
import {
  getDeviceScope,
  getMicId,
  getSpeakerId,
  setDeviceScope,
  setMicId,
  setSpeakerId,
} from "./devices";

describe("device preference scopes", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
  });

  it("resolves tab overrides before saved defaults", () => {
    setMicId("default-mic");
    setSpeakerId("default-speaker");
    expect(getMicId()).toBe("default-mic");
    expect(getSpeakerId()).toBe("default-speaker");

    setMicId("tab-mic", "tab");
    setSpeakerId("tab-speaker", "tab");
    expect(getDeviceScope()).toBe("tab");
    expect(getMicId()).toBe("tab-mic");
    expect(getSpeakerId()).toBe("tab-speaker");

    sessionStorage.clear();
    expect(getDeviceScope()).toBe("default");
    expect(getMicId()).toBe("default-mic");
    expect(getSpeakerId()).toBe("default-speaker");
  });

  it("promotes the current tab selection when saving as default", () => {
    setMicId("tab-mic", "tab");
    setSpeakerId("tab-speaker", "tab");
    setDeviceScope("default");
    expect(getDeviceScope()).toBe("default");
    expect(localStorage.getItem("cloudroom.micId")).toBe("tab-mic");
    expect(localStorage.getItem("cloudroom.spkId")).toBe("tab-speaker");
    expect(sessionStorage.getItem("cloudroom.micId")).toBeNull();
    expect(sessionStorage.getItem("cloudroom.spkId")).toBeNull();
  });
});
