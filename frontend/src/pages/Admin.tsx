import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { AuditEntry, GuestUpgradeRequest, Room, RoomRequest, api } from "../api";
import { useAuth } from "../auth";
import { InvitesTab } from "../components/InvitesTab";
import { UsersTab } from "../components/UsersTab";
import { EmojisTab } from "../components/EmojisTab";
import { useDialog } from "../hooks/useDialog";

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

type Tab = "overview" | "rooms" | "requests" | "upgrades" | "users" | "invites" | "emojis" | "audit";

const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "总览" },
  { id: "rooms", label: "房间" },
  { id: "requests", label: "申请" },
  { id: "upgrades", label: "转正" },
  { id: "users", label: "用户" },
  { id: "invites", label: "邀请" },
  { id: "emojis", label: "Emoji" },
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
      {tab === "upgrades" && <GuestUpgradeTab />}
      {tab === "users" && <UsersTab isSuper={isSuper} />}
      {tab === "invites" && <InvitesTab />}
      {tab === "emojis" && <EmojisTab />}
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
  const closeDetail = () => setDetail(null);
  const detailRef = useDialog<HTMLDivElement>(closeDetail);
  const [error, setError] = useState<string | null>(null);

  const searchTimer = useRef<number | null>(null);
  const searchAbort = useRef<AbortController | null>(null);

  const load = (kind: string, state: string, q: string, signal?: AbortSignal) => {
    const sp = new URLSearchParams();
    if (kind) sp.set("kind", kind);
    if (state) sp.set("state", state);
    if (q) sp.set("search", q);
    api<{ rooms: RoomRow[]; total: number }>(`/api/v1/admin/rooms?${sp.toString()}`, { signal })
      .then((r) => {
        setRows(r.rooms);
        setTotal(r.total);
      })
      .catch((e) => {
        // 被下一次输入取消的请求不算错误。
        if (e instanceof DOMException && e.name === "AbortError") return;
        setError(e.message);
      });
  };

  // 搜索防抖 + 取消上一次请求：避免每个按键一次查询、响应乱序覆盖。
  const scheduleSearch = (q: string) => {
    if (searchTimer.current) window.clearTimeout(searchTimer.current);
    searchTimer.current = window.setTimeout(() => {
      searchAbort.current?.abort();
      const ac = new AbortController();
      searchAbort.current = ac;
      load(kindFilter, stateFilter, q, ac.signal);
    }, 300);
  };

  const cancelSearch = () => {
    if (searchTimer.current) window.clearTimeout(searchTimer.current);
    searchAbort.current?.abort();
  };

  useEffect(
    () => () => {
      if (searchTimer.current) window.clearTimeout(searchTimer.current);
      searchAbort.current?.abort();
    },
    [],
  );

  useEffect(() => {
    load("", "", "");
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
          onChange={(e) => { setKindFilter(e.target.value); cancelSearch(); load(e.target.value, stateFilter, search); }}
        >
          <option value="">全部类型</option>
          <option value="standard">普通房间</option>
          <option value="direct">私聊</option>
        </select>
        <select value={stateFilter} onChange={(e) => { setStateFilter(e.target.value); cancelSearch(); load(kindFilter, e.target.value, search); }}>
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
            scheduleSearch(e.target.value);
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
          <div className="card" ref={detailRef} role="dialog" aria-modal="true" aria-label="房间详情">
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
      setRequests((prev) => prev.filter((r) => r.request_id !== id));
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
      setRequests((prev) => prev.filter((r) => r.request_id !== id));
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

function GuestUpgradeTab() {
  const [rows, setRows] = useState<GuestUpgradeRequest[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = async () => {
    try {
      const r = await api<{ requests: GuestUpgradeRequest[] }>(
        "/api/v1/admin/guest-upgrade-requests?status=pending",
      );
      setRows(r.requests);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const decide = async (id: string, action: "approve" | "reject") => {
    setBusy(id);
    try {
      await api(`/api/v1/admin/guest-upgrade-requests/${id}/${action}`, {
        method: "POST",
        body: "{}",
      });
      await load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <section>
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="data-table">
          <thead>
            <tr>
              <th>房间</th>
              <th>访客</th>
              <th>申请人</th>
              <th>提交时间</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.request_id}>
                <td>{r.room_code}</td>
                <td>{r.guest_username || r.guest_user_id.slice(0, 8)}</td>
                <td>{r.requester_username || "—"}</td>
                <td>{new Date(r.created_at).toLocaleString()}</td>
                <td className="row-actions">
                  <button disabled={busy === r.request_id} onClick={() => void decide(r.request_id, "approve")}>
                    批准
                  </button>
                  <button disabled={busy === r.request_id} onClick={() => void decide(r.request_id, "reject")}>
                    拒绝
                  </button>
                </td>
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td colSpan={5} className="hint">
                  暂无待审批的转正申请
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}
