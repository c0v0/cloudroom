import { describe, expect, it } from "vitest";

import type { Role } from "../api";
import { roleLabel } from "./roles";

describe("roleLabel", () => {
  it.each([
    ["super_admin", "超级管理员"],
    ["admin", "管理员"],
    ["user", "用户"],
    ["guest", "访客"],
  ] as const)("maps %s", (role, label) => {
    expect(roleLabel(role)).toBe(label);
  });

  it("returns undefined for an unknown runtime role", () => {
    expect(roleLabel("owner" as Role)).toBeUndefined();
  });
});
