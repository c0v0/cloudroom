import { FormEvent, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, Room, RoomRequest } from "../api";
import { useAuth } from "../auth";
import { normalizeRoomCode } from "../lib/roomCode";

export default function Home() {
  const { me } = useAuth();
  const [rooms, setRooms] = useState<Room[]>([]);
  const [requests, setRequests] = useState<RoomRequest[]>([]);
  const [note, setNote] = useState("");
  const [desiredCode, setDesiredCode] = useState("");
  const [mediaMode, setMediaMode] = useState<"p2p" | "sfu">("p2p");
  const [audioBitrate, setAudioBitrate] = useState(48);
  const [bitrateTiers, setBitrateTiers] = useState<
    { kbps: number; min_owner_role: string }[]
  >([]);
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const r = await api<{ rooms: Room[] }>("/api/v1/me/rooms");
      setRooms(r.rooms);
      const q = await api<{ requests: RoomRequest[] }>("/api/v1/room-requests");
      setRequests(q.requests);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    void load();
    api<{ bitrate_tiers: { kbps: number; min_owner_role: string }[] }>("/api/v1/voice/options")
      .then((r) => setBitrateTiers(r.bitrate_tiers || []))
      .catch(() => undefined);
  }, []);
  // 按账号角色过滤可用码率档位（房主可设档位由配置决定）。
  const roleRank = (role: string | undefined): number =>
    role === "super_admin" ? 3 : role === "admin" ? 2 : role === "user" ? 1 : 0;
  const availableTiers = bitrateTiers.filter(
    (t) => roleRank(me?.user.role) >= roleRank(t.min_owner_role),
  );

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setOk(null);
    try {
      const res = await api<{ auto_approved?: boolean; room?: { code: string } }>(
        "/api/v1/room-requests",
        {
          method: "POST",
          body: JSON.stringify({
            kind: "create_room",
            note: note || undefined,
            media_mode: mediaMode,
            audio_bitrate: mediaMode === "sfu" ? audioBitrate : 0,
            desired_code:
              me?.user.role === "super_admin" && desiredCode.trim()
                ? normalizeRoomCode(desiredCode)
                : undefined,
          }),
        }
      );
      setNote("");
      setDesiredCode("");
      if (res.auto_approved && res.room) {
        window.location.href = `/${res.room.code}`;
        return;
      }
      setOk("申请已提交，等待管理员审批。");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const cancel = async (id: string) => {
    try {
      await api(`/api/v1/room-requests/${id}/cancel`, { method: "POST", body: "{}" });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="page">
      <div className="page-head">
        <h1>我的房间</h1>
        <button className="link" onClick={() => void load()}>刷新</button>
      </div>
      {error && <p className="error">{error}</p>}
      {ok && <p className="ok">{ok}</p>}
      {rooms.length === 0 && <p className="hint">还没有房间——在下方提交申请。</p>}
      <ul className="room-list">
        {rooms.map((r) => (
          <li key={r.code}>
            <Link to={`/${r.code}`} className="room-row">
              <span className="code">{r.code}</span>
              <span className="tag">{r.kind === "direct" ? "私聊" : "房间"}</span>
              <span className="tag">
                {r.media_mode === "p2p" ? "P2P" : "SFU"}
              </span>
              <span className={`tag ${r.state === "active" ? "ok" : "warn"}`}>
                {r.state === "active" ? "正常" : r.state === "deleted" ? "已删除" : "已过期"}
              </span>
              <span className="muted">{r.my_role === "owner" ? "房主" : "成员"}</span>
            </Link>
          </li>
        ))}
      </ul>

      {me?.user.role === "guest" && (
        <section className="card">
          <h2>访客账号</h2>
          <p className="hint">访客不能申请建房，注册后即可申请。</p>
        </section>
      )}
      {me?.user.role !== "guest" && (
      <section className="card">
        <h2>申请创建房间</h2>
        <p className="hint">创建房间需选择媒体模式，提交后等待管理员审批。</p>
        <form onSubmit={submit}>
          <label>
            媒体模式
            <select value={mediaMode} onChange={(e) => setMediaMode(e.target.value as "p2p" | "sfu")}>
              <option value="p2p">P2P（浏览器直连）</option>
              <option value="sfu">SFU（LiveKit 转发）</option>
            </select>
          </label>
          {mediaMode === "sfu" && (
            <label>
              音频码率
              <select value={audioBitrate} onChange={(e) => setAudioBitrate(Number(e.target.value))}>
                {availableTiers.map((t) => (
                  <option key={t.kbps} value={t.kbps}>
                    {t.kbps} kbps
                  </option>
                ))}
              </select>
            </label>
          )}
          <label>
            用途 / 说明
            <textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              maxLength={500}
              placeholder="这个房间用来做什么？"
            />
          </label>
          {me?.user.role === "super_admin" && (
            <label>
              自定义房间号（仅超级管理员）
              <input
                value={desiredCode}
                onChange={(e) => setDesiredCode(e.target.value)}
                placeholder="例如 666 或 ccc — 3-32 位，仅 a-z0-9-"
              />
            </label>
          )}
          <button disabled={busy}>{busy ? "提交中…" : "提交申请"}</button>
        </form>
      </section>
      )}

      {requests.length > 0 && (
        <section>
          <h2>我的申请</h2>
          <ul className="room-list">
            {requests.map((r) => (
              <li key={r.request_id}>
                <span className="tag">{r.status}</span>
                <span className="tag">
                  {r.kind === "create_room" ? "创建房间" : "媒体模式变更"}
                </span>
                <span className="tag">{r.media_mode === "p2p" ? "P2P" : "SFU"}</span>
                {r.kind === "create_room" && r.desired_code && (
                  <span className="tag">code: {r.desired_code}</span>
                )}
                {r.kind === "change_media_mode" && r.target_room_code && (
                  <span className="tag">房间: {r.target_room_code}</span>
                )}{" "}
                {r.note || "（无说明）"}
                {r.status === "pending" && (
                  <button className="link" onClick={() => void cancel(r.request_id)}>
                    取消
                  </button>
                )}
                {r.status === "rejected" && r.decision_reason && (
                  <span className="muted"> — {r.decision_reason}</span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
