import { beforeEach, describe, expect, it } from "vitest";
import { getGateDb, getGateHoldMs, setGateDb, setGateHoldMs } from "./micGate";

describe("mic gate preferences", () => {
  beforeEach(() => localStorage.clear());

  it("stores and clears the gate threshold", () => {
    expect(getGateDb()).toBeNull();
    setGateDb(-50);
    expect(getGateDb()).toBe(-50);
    setGateDb(null);
    expect(getGateDb()).toBeNull();
  });

  it("clamps thresholds to the supported dBFS range", () => {
    setGateDb(-80);
    expect(getGateDb()).toBe(-60);
    setGateDb(-10);
    expect(getGateDb()).toBe(-20);
  });

  it("stores and clamps the close delay", () => {
    expect(getGateHoldMs()).toBe(250);
    setGateHoldMs(500);
    expect(getGateHoldMs()).toBe(500);
    setGateHoldMs(-20);
    expect(getGateHoldMs()).toBe(0);
    setGateHoldMs(2000);
    expect(getGateHoldMs()).toBe(1000);
  });
});
