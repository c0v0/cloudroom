import { describe, expect, it } from "vitest";
import { levelsChanged } from "./volumeMeter";

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
