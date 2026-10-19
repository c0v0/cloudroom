import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { AuditEntry, Invite, Room, RoomRequest, User, api } from "../api";
import { useAuth } from "../auth";
import { roleLabel } from "../lib/roles";

interface Stats {
  users: number;
  rooms: number;
  rooms_active: number;
  rooms_deleted: number;
  rooms_expired: number;
  room_requests_pending: number;
  messages: number;
}

interface RoomRow {
  room: Room;
  member_count: number;
  owner_username: string | null;
}

type Tab = "overview" | "rooms" | "requests" | "users" | "invites" | "audit";

const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "总览" },
  { id: "rooms", label: "房间" },
  { id: "requests", label: "申请" },
  { id: "users", label: "用户" },
  { id: "invites", label: "邀请" },
  { id: "audit", label: "审计" },
];

export default function Admin() {
  const [tab, setTab] = useState<Tab>("overview");
  const { me } = useAuth();
  const isSuper = me?.user.role === "super_admin";

  return (
    <div className="page">
      <h1>管理后台</h1>
      <nav className="tabs">
        {TABS.map((t) => (
          <button key={t.id} className={tab === t.id ? "active" : ""} onClick={() => setTab(t.id)}>
            {t.label}
          </button>
        ))}
      </nav>
      {tab === "overview" && <Overview />}
      {tab === "rooms" && <RoomsTab isSuper={isSuper} />}
      {tab === "requests" && <RequestsTab />}
      {tab === "users" && <UsersTab isSuper={isSuper} />}
      {tab === "invites" && <InvitesTab />}
      {tab === "audit" && <AuditTab />}
    </div>
  );
}

function Overview() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api<Stats>("/api/v1/admin/stats")
      .then(setStats)
      .catch((e) => setError(e.message));
  }, []);
  if (error) return <p className="error">{error}</p>;
  if (!stats) return <p className="hint">加载中…</p>;
  return (
    <div className="stats">
      <div className="stat">
        <b>{stats.users}</b> 用户
      </div>
      <div className="stat">
        <b>{stats.rooms_active}</b> 活跃房间
      </div>
      <div className="stat">
        <b>{stats.rooms_deleted}</b> 已删除
      </div>
      <div className="stat">
        <b>{stats.rooms_expired}</b> 已过期
      </div>
      <div className="stat">
        <b>{stats.room_requests_pending}</b> 待审批申请
      </div>
      <div className="stat">
        <b>{stats.messages}</b> 条已存消息
      </div>
    </div>
  );
}

function RoomsTab({ isSuper }: { isSuper: boolean }) {
  const [rows, setRows] = useState<RoomRow[]>([]);
  const [total, setTotal] = useState(0);
  const [kindFilter, setKindFilter] = useState("");
  const [stateFilter, setStateFilter] = useState("");
  const [search, setSearch] = useState("");
  const [detail, setDetail] = useState<{ room: Room; members: Member[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = (kind: string, state: string, q: string) => {
    const sp = new URLSearchParams();
    if (kind) sp.set("kind", kind);
    if (state) sp.set("state", state);
    if (q) sp.set("search", q);
    api<{ rooms: RoomRow[]; total: number }>(`/api/v1/admin/rooms?${sp.toString()}`)
      .then((r) => {
        setRows(r.rooms);
        setTotal(r.total);
      })
      .catch((e) => setError(e.message));
  };

  useEffect(() => {
    load("", "", "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const 查看 = async (code: string) => {
    try {
      const r = await api<{ room: Room; members: Member[] }>(`/api/v1/admin/rooms/${code}`);
      setDetail(r);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const forceDelete = async (code: string) => {
    if (!window.confirm(`强制删除房间 ${code}？房间号将永久作废。`)) return;
    try {
      await api(`/api/v1/admin/rooms/${code}/delete`, { method: "POST", body: "{}" });
      load(kindFilter, stateFilter, search);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const transfer = async (code: string) => {
    const uid = window.prompt(`将房间 ${code} 转让给用户 ID：`);
    if (!uid) return;
    try {
      await api(`/api/v1/admin/rooms/${code}/transfer-owner`, {
        method: "POST",
        body: JSON.stringify({ user_id: uid }),
      });
      load(kindFilter, stateFilter, search);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section>
      <div className="filters">
        <select
          value={kindFilter}
          onChange={(e) => { setKindFilter(e.target.value); load(e.target.value, stateFilter, search); }}
        >
          <option value="">全部类型</option>
          <option value="standard">普通房间</option>
          <option value="direct">私聊</option>
        </select>
        <select value={stateFilter} onChange={(e) => { setStateFilter(e.target.value); load(kindFilter, e.target.value, search); }}>
          <option value="">全部状态</option>
          <option value="active">正常</option>
          <option value="deleted">已删除</option>
          <option value="expired">已过期</option>
        </select>
        <input
          placeholder="搜索房间号"
          value={search}
          onChange={(e) => {
            setSearch(e.target.value);
            load(kindFilter, stateFilter, e.target.value);
          }}
        />
      </div>
      <p className="hint">共 {total} 个房间</p>
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>房间号</th>
            <th>类型</th>
            <th>媒体</th>
            <th>状态</th>
            <th>房主</th>
            <th>成员数</th>
            <th>创建时间</th>
            <th>过期时间</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.room.code}>
              <td>
                <Link to={`/${r.room.code}`}>{r.room.code}</Link>
              </td>
              <td>{r.room.kind === "direct" ? "私聊" : "房间"}</td>
              <td>
                <span className="tag">{r.room.media_mode === "p2p" ? "P2P" : "SFU"}</span>
              </td>
              <td>
                <span className={`tag ${r.room.state === "active" ? "ok" : "warn"}`}>
                  {r.room.state === "active" ? "正常" : r.room.state === "deleted" ? "已删除" : "已过期"}
                </span>
              </td>
              <td>{r.owner_username || r.room.owner_user_id?.slice(0, 8) || "—"}</td>
              <td>{r.member_count}</td>
              <td>{new Date(r.room.created_at).toLocaleString()}</td>
              <td>{r.room.expires_at ? new Date(r.room.expires_at).toLocaleString() : "永久"}</td>
              <td className="row-actions">
                <button onClick={() => void 查看(r.room.code)}>查看</button>
                {r.room.state === "active" && (
                  <button className="danger" onClick={() => void forceDelete(r.room.code)}>
                    删除
                  </button>
                )}
                {isSuper && r.room.kind === "standard" && r.room.state === "active" && (
                  <button onClick={() => void transfer(r.room.code)}>转让房主</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
      {detail && (
        <div className="modal-shell">
          <div className="card">
            <h2>房间 /{detail.room.code}</h2>
            <p>
              {detail.room.kind === "direct" ? "私聊" : "房间"} ·{" "}
              {detail.room.state === "active" ? "正常" : detail.room.state === "deleted" ? "已删除" : "已过期"} · 文字{" "}
              {detail.room.text_enabled ? "开启" : "关闭"} · 媒体{" "}
              {detail.room.media_mode === "p2p" ? "P2P" : "SFU"}
            </p>
            <ul className="members">
              {detail.members.map((m) => (
                <li key={m.user_id}>
                  {m.nickname || m.username || m.user_id.slice(0, 8)}{" "}
                  <span className="tag">{m.room_role === "owner" ? "房主" : "成员"}</span>
                </li>
              ))}
            </ul>
            <button onClick={() => setDetail(null)}>关闭</button>
          </div>
        </div>
      )}
    </section>
  );
}

interface Member {
  user_id: string;
  username: string | null;
  nickname: string;
  room_role: string;
  joined_at: string;
  last_online_at: string | null;
}

function RequestsTab() {
  const [requests, setRequests] = useState<RoomRequest[]>([]);
  const [reasons, setReasons] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    api<{ requests: RoomRequest[] }>("/api/v1/admin/room-requests?status=pending")
      .then((r) => setRequests(r.requests))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);

  const approve = async (id: string) => {
    try {
      await api(`/api/v1/admin/room-requests/${id}/approve`, { method: "POST", body: "{}" });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const reject = async (id: string) => {
    const reason = reasons[id] || "";
    if (!reason) {
      setError("必须填写拒绝原因（将写入审计日志）。");
      return;
    }
    try {
      await api(`/api/v1/admin/room-requests/${id}/reject`, {
        method: "POST",
        body: JSON.stringify({ reason }),
      });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section>
      {error && <p className="error">{error}</p>}
      {requests.length === 0 && <p className="hint">没有待审批的申请。</p>}
      {requests.map((r) => (
        <div key={r.request_id} className="card request-row">
          <p>
            <b>{r.requester_username || r.requester_id.slice(0, 8)}</b>{" "}
            {r.kind === "create_room" ? "申请创建房间" : "申请变更媒体模式"} ·{" "}
            {new Date(r.created_at).toLocaleString()}
            <span className="tag">模式：{r.media_mode === "p2p" ? "P2P" : "SFU"}</span>
            {r.kind === "create_room" && (
              <span className="tag">房间号：{r.desired_code || "自动生成"}</span>
            )}
            {r.kind === "change_media_mode" && r.target_room_code && (
              <span className="tag">房间：{r.target_room_code}</span>
            )}
          </p>
          {r.note && <p className="muted">{r.note}</p>}
          <div className="row-actions">
            <button className="ok" onClick={() => void approve(r.request_id)}>
              同意申请
            </button>
            <input
              placeholder="拒绝原因（必填）"
              value={reasons[r.request_id] || ""}
              onChange={(e) => setReasons((x) => ({ ...x, [r.request_id]: e.target.value }))}
            />
            <button className="danger" onClick={() => void reject(r.request_id)}>
              拒绝
            </button>
          </div>
        </div>
      ))}
    </section>
  );
}

function UsersTab({ isSuper }: { isSuper: boolean }) {
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [tempPw, setTempPw] = useState<string | null>(null);

  const load = () => {
    api<{ users: User[] }>("/api/v1/admin/users")
      .then((r) => setUsers(r.users))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);

  const setDisabled = async (uid: string, disabled: boolean) => {
    try {
      await api(`/api/v1/admin/users/${uid}/disable`, {
        method: "POST",
        body: JSON.stringify({ disabled }),
      });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const resetPw = async (uid: string) => {
    if (!window.confirm("重置该用户密码？其所有会话将立即失效。")) return;
    try {
      const r = await api<{ temporary_password: string }>(`/api/v1/admin/users/${uid}/reset-password`, {
        method: "POST",
        body: "{}",
      });
      setTempPw(r.temporary_password);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const setRole = async (uid: string, role: string) => {
    try {
      await api(`/api/v1/admin/users/${uid}/role`, {
        method: "POST",
        body: JSON.stringify({ role }),
      });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section>
      {error && <p className="error">{error}</p>}
      {tempPw && (
        <p className="ok">
          临时密码（仅显示一次）：<code>{tempPw}</code>
        </p>
      )}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>用户名</th>
            <th>角色</th>
            <th>创建时间</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.user_id}>
              <td>{u.username}</td>
              <td>{roleLabel(u.role)}</td>
              <td>{new Date(u.created_at).toLocaleString()}</td>
              <td>{u.disabled ? <span className="tag warn">已禁用</span> : <span className="tag ok">正常</span>}</td>
              <td className="row-actions">
                <button onClick={() => void resetPw(u.user_id)}>重置密码</button>
                {u.role !== "super_admin" && (
                  <button onClick={() => void setDisabled(u.user_id, !u.disabled)}>
                    {u.disabled ? "启用" : "禁用"}
                  </button>
                )}
                {isSuper && u.role !== "super_admin" && (
                  <select value={u.role} onChange={(e) => void setRole(u.user_id, e.target.value)}>
                    <option value="user">普通用户</option>
                    <option value="admin">管理员</option>
                  </select>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
    </section>
  );
}

function InvitesTab() {
  const [invites, setInvites] = useState<Invite[]>([]);
  const [created, setCreated] = useState<{ url: string; token: string } | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    api<{ invites: Invite[] }>("/api/v1/admin/invites")
      .then((r) => setInvites(r.invites))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);

  const create = async () => {
    try {
      const r = await api<{ token: string; register_url: string }>("/api/v1/admin/invites", {
        method: "POST",
        body: "{}",
      });
      setCreated({ url: location.origin + r.register_url, token: r.token });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const 撤销 = async (id: string) => {
    try {
      await api(`/api/v1/admin/invites/${id}/revoke`, { method: "POST", body: "{}" });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <section>
      <button onClick={() => void create()}>生成注册邀请</button>
      {created && (
        <p className="ok">
          注册链接（仅显示一次）：<code>{created.url}</code>
          <span className="row-actions">
            <button
              onClick={() => {
                void navigator.clipboard.writeText(created.url).then(
                  () => setCopied("链接"),
                  () => setCopied(null)
                );
              }}
            >
              复制链接
            </button>
            <button
              onClick={() => {
                void navigator.clipboard.writeText(created.token).then(
                  () => setCopied("邀请码"),
                  () => setCopied(null)
                );
              }}
            >
              复制邀请码
            </button>
            {copied && <span className="tag ok">已复制{copied}</span>}
          </span>
        </p>
      )}
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>创建时间</th>
            <th>过期时间</th>
            <th>使用者</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {invites.map((i) => (
            <tr key={i.invite_id}>
              <td>{new Date(i.created_at).toLocaleString()}</td>
              <td>{new Date(i.expires_at).toLocaleString()}</td>
              <td>{i.consumed_by ? i.consumed_by.slice(0, 8) : "—"}</td>
              <td>
                <span className={`tag ${i.active ? "ok" : "warn"}`}>
                  {i.consumed_at ? "已使用" : i.revoked_at ? "已撤销" : i.active ? "正常" : "已过期"}
                </span>
              </td>
              <td>
                {i.active && (
                  <button onClick={() => void 撤销(i.invite_id)}>撤销</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
    </section>
  );
}

const AUDIT_LABELS: Record<string, string> = {
  "room_request.created": "提交建房申请",
  "room_request.approved": "批准建房申请",
  "room_request.rejected": "拒绝建房申请",
  "room_request.cancelled": "取消建房申请",
  "room_media_mode.changed": "媒体模式变更",
  "room.settings_updated": "修改房间设置",
  "room.deleted": "删除房间",
  "room.expired": "房间到期",
  "room.direct_idle_deleted": "私聊超时删除",
  "room.purged": "物理清理房间",
  "room.owner_transferred": "转让房主",
  "room.left": "成员离开房间",
  "user.registered": "用户注册",
  "user.disabled": "禁用用户",
  "user.enabled": "启用用户",
  "user.password_reset": "重置用户密码",
  "user.password_changed": "修改密码",
  "user.role_changed": "修改用户角色",
  "user.avatar_updated": "更新头像",
  "guest.upgraded": "访客升级",
  "invite.registration_created": "生成注册邀请",
  "invite.registration_revoked": "撤销注册邀请",
  "invite.created": "生成房间邀请",
  "invite.revoked": "撤销房间邀请",
  "invite.redeemed": "兑换邀请",
  "member.role_changed": "修改成员角色",
  "member.nickname_changed": "修改成员昵称",
  "member.removed": "移出成员",
  "mute.created": "发起禁言",
  "mute.removed": "解除禁言",
};

function AuditTab() {
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const load = () => {
    api<{ entries: AuditEntry[] }>("/api/v1/admin/audit")
      .then((r) => setEntries(r.entries))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);
  return (
    <section>
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>时间</th>
            <th>操作者</th>
            <th>动作</th>
            <th>目标</th>
            <th>详情</th>
          </tr>
        </thead>
        <tbody>
          {entries.map((e) => (
            <tr key={e.entry_id}>
              <td>{new Date(e.created_at).toLocaleString()}</td>
              <td>{e.actor_user_id?.slice(0, 8) || "系统"}</td>
              <td>{AUDIT_LABELS[e.action] || e.action}</td>
              <td>
                {e.target_type === "room" ? "房间" : e.target_type === "user" ? "用户" : e.target_type === "invite" ? "邀请" : e.target_type === "room_request" ? "申请" : e.target_type || "—"}{" "}
                {e.target_id?.slice(0, 8)}
              </td>
              <td className="muted">{JSON.stringify(e.detail)}</td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
    </section>
  );
}
