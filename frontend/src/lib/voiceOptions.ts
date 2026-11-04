/** 码率档位（含房主角色门槛）：模块级 Promise 缓存，首页与房间共用。 */
import { api } from "../api";

export interface BitrateTier {
  kbps: number;
  min_owner_role: string;
}

let pending: Promise<BitrateTier[]> | null = null;

export function loadBitrateTiers(): Promise<BitrateTier[]> {
  pending ??= api<{ bitrate_tiers: BitrateTier[] }>("/api/v1/voice/options")
    .then((r) => r.bitrate_tiers || [])
    .catch((e: unknown) => {
      pending = null;
      throw e;
    });
  return pending;
}
