import { beforeEach, describe, expect, it } from "vitest";
import { getGateDb, setGateDb } from "./micGate";

describe("mic gate preference", () => {
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
});
