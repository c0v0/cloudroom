import type { Role } from "../api";

export function roleLabel(role: Role): string {
  switch (role) {
    case "super_admin":
      return "超级管理员";
    case "admin":
      return "管理员";
    case "user":
      return "用户";
    case "guest":
      return "访客";
  }
}
