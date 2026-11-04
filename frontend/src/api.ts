// CloudRoom frontend API client. Same-origin fetch with cookies; mutations
// carry the session-bound CSRF token. No tokens in localStorage/URLs.

export type Role = "super_admin" | "admin" | "user" | "guest";

export interface User {
  user_id: string;
  username: string | null;
  role: Role;
  avatar_url: string | null;
  created_at: string;
  disabled: boolean;
  /** 邀请人用户名（仅管理端下发）。 */
  invited_by?: string | null;
}

export interface MeResponse {
  user: User;
  csrf_token: string;
  session_expires_at: string;
}

export interface PublicConfig {
  registration_mode: "invite_only" | "open";
  bootstrap_pending: boolean;
  features: {
    voice: boolean;
    screen_share: boolean;
    media_playback: boolean;
    voice_modes: ("p2p" | "sfu")[];
    p2p_voice_max_participants: number;
  };
  limits: {
    max_avatar_bytes: number;
    max_avatar_dimension: number;
    max_nickname_graphemes: number;
    max_message_chars: number;
  };
  rooms: { generated_code_length: number };
}

export interface Room {
  code: string;
  kind: "standard" | "direct";
  state: "active" | "expired" | "deleted";
  owner_user_id: string | null;
  created_at: string;
  expires_at: string | null;
  allow_direct_messages: boolean;
  text_enabled: boolean;
  media_mode: "p2p" | "sfu";
  /** 音频码率（kbps，0=自动）。仅 SFU 生效。 */
  audio_bitrate: number;
  /** 房间级全员禁言（仅房主可开关）。 */
  room_muted: boolean;
  /** 房间公告（房主设置，顶部横幅展示）。 */
  announcement: string;
  /** 成员上限（0=不限）。 */
  max_members: number;
  last_seq: number;
  my_role: string | null;
  /** 媒体加密（服务器持钥）（仅 SFU 生效）。 */
  e2ee: boolean;
  /** 是否允许消息附件（建房申请时确定）。 */
  attachments_enabled: boolean;
  /** 房主为管理员时可开启：房间内访客可直接注册。 */
  guest_upgrade_open?: boolean;
}

/** 房间语音权限状态（get_room 的 voice 字段）。 */
export interface RoomVoice {
  can_speak: boolean;
  granted: boolean;
  pending: boolean;
  /** 房主可见：已获发言权用户。 */
  grants?: { user_id: string; nickname: string; granted_at: string }[];
  /** 房主可见：待审批的发言申请。 */
  requests?: { user_id: string; nickname: string; created_at: string }[];
}

export interface Member {
  user_id: string;
  username: string | null;
  avatar_url: string | null;
  nickname: string;
  room_role: "owner" | "member";
  joined_at: string;
  last_online_at: string | null;
}

export interface MessageReaction {
  emoji: string;
  count: number;
  me: boolean;
}

export interface Message {
  id: string;
  seq: number;
  sender: { user_id: string | null; username: string | null; nickname: string | null } | null;
  body: string;
  created_at: string;
  /** 回复引用的原消息快照（{id, sender, uid, body}）。 */
  reply?: { id: string; sender: string; uid: string | null; body: string } | null;
  /** emoji 回应聚合（历史加载时附带；实时事件替换）。 */
  reactions?: MessageReaction[];
  /** 编辑时间（ISO）；存在即表示已编辑。 */
  edited_at?: string | null;
  /** 软删除：正文不再下发，前端渲染占位。 */
  deleted?: boolean;
  attachment_id?: string | null;
  /** 附件元数据（图片/文件）。 */
  attachment?: Attachment | null;
}

export interface Attachment {
  id: string;
  filename: string;
  mime: string;
  size: number;
  /** 相对 API 路径；跨域部署时用 assetUrl() 补前缀。 */
  url: string;
}

export interface Mute {
  mute_id: string;
  scope: "member" | "room";
  target_user_id: string | null;
  muted_by: string;
  created_at: string;
  expires_at: string | null;
}

export interface RoomRequest {
  request_id: string;
  requester_id: string;
  requester_username?: string | null;
  kind: "create_room" | "change_media_mode";
  note: string;
  desired_code: string | null;
  target_room_code: string | null;
  media_mode: "p2p" | "sfu";
  /** 建房申请携带的音频码率（kbps，0=自动）。 */
  audio_bitrate: number;
  status: string;
  created_at: string;
  decided_at: string | null;
  decided_by: string | null;
  decision_reason: string | null;
}

export interface GuestUpgradeRequest {
  request_id: string;
  room_id: string;
  room_code: string;
  guest_user_id: string;
  guest_username: string;
  requested_by: string;
  requester_username: string;
  status: string;
  note: string;
  created_at: string;
  decided_at: string | null;
  decided_by: string | null;
}

export interface Invite {
  invite_id: string;
  kind: string;
  max_role: string;
  created_at: string;
  expires_at: string;
  consumed_at: string | null;
  consumed_by: string | null;
  revoked_at: string | null;
  active: boolean;
  token?: string;
}

export interface AuditEntry {
  entry_id: number;
  actor_user_id: string | null;
  action: string;
  target_type: string | null;
  target_id: string | null;
  detail: Record<string, unknown>;
  created_at: string;
}

export interface ApiError {
  error: { code: string; message: string };
}

let csrfCache: string | null = null;
let meCache: MeResponse | null = null;

const configuredApiOrigin = (): string => {
  const raw = import.meta.env.VITE_API_ORIGIN as string | undefined;
  return raw ? raw.replace(/\/+$/, "") : "";
};

const apiUrl = (path: string): string => {
  if (/^https?:\/\//i.test(path)) return path;
  return `${configuredApiOrigin()}${path}`;
};

export async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers: Record<string, string> = {
    ...(init.headers as Record<string, string> | undefined),
  };
  if (init.body && !(init.body instanceof FormData) && !headers["Content-Type"]) {
    headers["Content-Type"] = "application/json";
  }
  const method = (init.method || "GET").toUpperCase();
  if (method !== "GET" && method !== "HEAD") {
    const csrf = await getCsrf();
    if (csrf) headers["X-CSRF-Token"] = csrf;
  }
  // 默认 20s 超时：网络挂起时 UI 不会永远停在加载态。
  const controller = new AbortController();
  const timer = window.setTimeout(() => controller.abort(), 20_000);
  let res: Response;
  try {
    res = await fetch(apiUrl(path), {
      ...init,
      headers,
      credentials: configuredApiOrigin() ? "include" : "same-origin",
      signal: init.signal ?? controller.signal,
    });
  } catch (e) {
    if (controller.signal.aborted) {
      const err = new Error("请求超时，请检查网络后重试。") as Error & { status: number };
      err.status = 0;
      throw err;
    }
    throw e;
  } finally {
    window.clearTimeout(timer);
  }
  if (res.status === 401 && path !== "/api/v1/auth/login" && path !== "/api/v1/auth/register") {
    meCache = null;
    csrfCache = null;
    window.dispatchEvent(new CustomEvent("cloudroom:unauthorized"));
  }
  if (!res.ok) {
    let code = "error";
    let message = `HTTP ${res.status}`;
    try {
      const body = (await res.json()) as ApiError;
      code = body.error.code;
      message = body.error.message;
    } catch {
      /* non-JSON error body */
    }
    const err = new Error(message) as Error & { status: number; code: string };
    err.status = res.status;
    err.code = code;
    throw err;
  }
  return res.json() as Promise<T>;
}

export async function getCsrf(): Promise<string | null> {
  if (csrfCache) return csrfCache;
  try {
    const me = await api<MeResponse>("/api/v1/me");
    meCache = me;
    csrfCache = me.csrf_token;
    return me.csrf_token;
  } catch {
    return null;
  }
}

export async function me(): Promise<MeResponse | null> {
  if (meCache) return meCache;
  try {
    const m = await api<MeResponse>("/api/v1/me");
    meCache = m;
    csrfCache = m.csrf_token;
    return m;
  } catch {
    return null;
  }
}

export function invalidateMe(): void {
  meCache = null;
  csrfCache = null;
}

export const wsUrl = (roomCode: string): string => {
  const base = configuredApiOrigin();
  if (base) return base.replace(/^http/, "ws") + `/api/v1/rooms/${roomCode}/ws`;
  return `${location.origin.replace(/^http/, "ws")}/api/v1/rooms/${roomCode}/ws`;
};

export const apiOrigin = (): string => configuredApiOrigin();

export const assetUrl = (path: string | null): string | null => {
  if (!path) return null;
  if (/^(https?:|data:|blob:)/i.test(path)) return path;
  return `${configuredApiOrigin()}${path}`;
};
