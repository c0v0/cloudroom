import { describe, expect, it } from "vitest";
import {
  dbToThreshold,
  levelsChanged,
  thresholdToDb,
  VAD_MAX_DB,
  VAD_MIN_DB,
} from "./volumeMeter";

describe("levelsChanged", () => {
  it("notifies when a level crosses to zero even below epsilon", () => {
    expect(levelsChanged({ u1: 0.019 }, { u1: 0 })).toBe(true);
  });

  it("notifies when a level rises from zero", () => {
    expect(levelsChanged({ u1: 0 }, { u1: 0.004 })).toBe(true);
  });

  it("does not notify for tiny non-zero changes", () => {
    expect(levelsChanged({ u1: 0.5 }, { u1: 0.505 })).toBe(false);
  });
});

describe("说话动效阈值", () => {
  it("covers the full 24 dB slider range", () => {
    expect(thresholdToDb(dbToThreshold(VAD_MIN_DB))).toBe(VAD_MIN_DB);
    expect(thresholdToDb(dbToThreshold(VAD_MAX_DB))).toBe(VAD_MAX_DB);
    expect(dbToThreshold(VAD_MIN_DB)).toBeCloseTo(0.0526, 3);
    expect(dbToThreshold(VAD_MAX_DB)).toBeCloseTo(0.6842, 3);
  });

  it("keeps the legacy default at roughly the same sensitivity", () => {
    expect(thresholdToDb(0.05)).toBe(-48);
    expect(dbToThreshold(-48)).toBeCloseTo(0.0526, 3);
  });
});
