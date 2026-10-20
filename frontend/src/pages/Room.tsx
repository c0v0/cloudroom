import React, { useCallback, useEffect, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { api, Member, Message, MessageReaction, Mute, Room, RoomRequest, RoomVoice } from "../api";
import { useAuth } from "../auth";
import { useRoomSocket } from "../hooks/useRoomSocket";
import type { RoomSocket, RoomSocketEvent } from "../hooks/useRoomSocket";
import { useVoiceSession } from "../hooks/useVoiceSession";
import type { SignalData } from "../hooks/useP2PVoice";
import { playChime, playMessageChime } from "../hooks/chime";
import { throttleLeading, throttleTrailing } from "../hooks/throttle";
import { useMicTest } from "../hooks/useMicTest";
import { getVolumePref } from "../hooks/remoteAudio";

export default function RoomPage() {
  const { code = "" } = useParams();
  const [params] = useSearchParams();
  const inviteToken = params.get("invite");
  const { me, refresh } = useAuth();

  const [room, setRoom] = useState<Room | null>(null);
  const [members, setMembers] = useState<Member[]>([]);
  const [mutes, setMutes] = useState<Mute[]>([]);
  const [caps, setCaps] = useState<string[]>([]);
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [mentionQuery, setMentionQuery] = useState("");
  const [replyingTo, setReplyingTo] = useState<Message | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [notMember, setNotMember] = useState(false);
  const [joinToken, setJoinToken] = useState(inviteToken || "");
  const [joinBusy, setJoinBusy] = useState(false);
  const [linkCopied, setLinkCopied] = useState(false);
  const [speaking, setSpeaking] = useState<Record<string, number>>({});
  const [mediaStates, setMediaStates] = useState<
    Record<string, { mic: boolean; speaker: boolean }>
  >({});
  const [voiceInfo, setVoiceInfo] = useState<RoomVoice>({
    can_speak: true,
    granted: false,
    pending: false,
  });
  const prevVoiceRef = useRef<string[]>([]);

  const lastSeqRef = useRef(0);
  const myUserId = me?.user.user_id;
  // 新消息提示音开关（个人设置）。
  const soundEnabledRef = useRef(false);
  soundEnabledRef.current = (() => {
    try {
      return localStorage.getItem("cloudroom.sound") === "1";
    } catch {
      return false;
    }
  })();

  // 房间静音（按房间记忆：不通知不响铃）。
  const [roomMutedNotif, setRoomMutedNotif] = useState(() => {
    try {
      const all = JSON.parse(localStorage.getItem("cloudroom.mutedRooms") || "{}") as Record<string, boolean>;
      return !!all[code];
    } catch {
      return false;
    }
  });
  const roomMutedPrefRef = useRef(roomMutedNotif);
  roomMutedPrefRef.current = roomMutedNotif;
  const toggleRoomMutedNotif = () => {
    const next = !roomMutedNotif;
    setRoomMutedNotif(next);
    try {
      const all = JSON.parse(localStorage.getItem("cloudroom.mutedRooms") || "{}") as Record<string, boolean>;
      all[code] = next;
      localStorage.setItem("cloudroom.mutedRooms", JSON.stringify(all));
    } catch {
      /* 忽略 */
    }
  };

  const loadRoom = useCallback(async () => {
    try {
      const r = await api<{
        room: Room;
        members: Member[];
        mutes: Mute[];
        capabilities: string[];
        voice?: RoomVoice;
      }>(`/api/v1/rooms/${code}`);
      setRoom(r.room);
      setMembers(r.members);
      setMutes(r.mutes);
      setCaps(r.capabilities);
      if (r.voice) setVoiceInfo(r.voice);
      setNotMember(false);
      lastSeqRef.current = r.room.last_seq;
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 403) setNotMember(true);
      else setError(err.message);
    }
  }, [code]);

  const loadMessages = useCallback(async () => {
    try {
      const r = await api<{ messages: Message[]; last_seq: number }>(
        `/api/v1/rooms/${code}/messages?after=0`,
      );
      setMessages(r.messages);
      lastSeqRef.current = Math.max(lastSeqRef.current, r.last_seq);
    } catch {
      /* room gone */
    }
  }, [code]);

  useEffect(() => {
    void loadRoom();
    void loadMessages();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  // 邀请 URL：打开即自动进房（未注册用户同样适用）
  useEffect(() => {
    if (notMember && inviteToken && !joinBusy) {
      void join(inviteToken);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [notMember, inviteToken]);

  const join = async (token: string) => {
    setJoinBusy(true);
    setError(null);
    try {
      await api(`/api/v1/rooms/${code}/join`, {
        method: "POST",
        body: JSON.stringify({ invite_token: token || undefined }),
      });
      if (params.get("invite")) history.replaceState(null, "", `/${code}`);
      setJoinToken("");
      await refresh(); // 未注册用户此刻获得 guest 会话，刷新登录态
      await loadRoom();
      await loadMessages();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setJoinBusy(false);
    }
  };

  // P2P SDP/ICE 由控制面 WSS 中继；handleSignal 由语音会话提供（通过 ref 注入）。
  const signalRef = useRef<(from: string, data: SignalData) => void>(() => undefined);
  const prevModeRef = useRef<Room["media_mode"] | null>(null);
  const voiceMicMutedRef = useRef(false);

  const onSocketEvent = useCallback(
    (e: RoomSocketEvent) => {
      switch (e.type) {
        case "hello": {
          const m = e.members as Member[];
          if (Array.isArray(m)) setMembers(m);
          lastSeqRef.current = Math.max(
            lastSeqRef.current,
            (e as { last_seq?: number }).last_seq || 0,
          );
          break;
        }
        case "message": {
          const m = (e as unknown as { message: Message }).message;
          setMessages((prev) => (prev.some((x) => x.id === m.id) ? prev : [...prev, m]));
          lastSeqRef.current = Math.max(lastSeqRef.current, m.seq);
          const from = m.sender?.nickname || m.sender?.username || m.sender?.user_id?.slice(0, 8) || "";
          const muted = roomMutedPrefRef.current;
          // 桌面通知：房间不在前台时新消息提醒；被 @ / 被回复时同样提醒。
          if (from && !muted) {
            pushNotify(`房间 /${code}`, `${from}：${m.body.slice(0, 60)}`);
          }
          // 被回复提醒（即使房间静音也提醒，回复是直接对话）。
          if (m.reply?.uid && m.reply.uid === myUserId && from) {
            pushNotify(`房间 /${code}`, `${from} 回复了你：${m.body.slice(0, 60)}`);
          }
          // 新消息提示音（个人设置开关；房间静音时不响）。
          if (from && !muted && soundEnabledRef.current) {
            playMessageChime();
          }
          break;
        }
        case "member_joined":
        case "member_left": {
          const uid = e.user_id as string;
          setMembers((prev) => {
            if (e.type === "member_left") return prev.filter((m) => m.user_id !== uid);
            const m = e.member as Member;
            return prev.some((x) => x.user_id === uid) ? prev : [...prev, m];
          });
          break;
        }
        // 断连（关闭标签页）≠ 离开房间：成员关系保留，无需改成员列表。
        case "member_offline":
          break;
        case "member_updated": {
          const uid = e.user_id as string;
          setMembers((prev) =>
            prev.map((m) =>
              m.user_id === uid
                ? {
                    ...m,
                    nickname: (e.nickname as string) || m.nickname,
                    room_role: (e.room_role as Member["room_role"]) || m.room_role,
                    avatar_url: (e.avatar_url as string | null) ?? m.avatar_url,
                  }
                : m,
            ),
          );
          break;
        }
        case "room_settings": {
          const r = e.room as Room;
          setRoom((prev) => {
            if (prev && r.media_mode && r.media_mode !== prev.media_mode) {
              setNotice("房间媒体模式已变更，请重新加入语音。");
            }
            return prev ? { ...prev, ...r } : prev;
          });
          break;
        }
        case "muted": {
          if (e.target_user_id === myUserId) {
            setNotice("您已被管理员禁言。");
            if (voiceMicMutedRef.current === false) {
              voiceMicMutedRef.current = true;
            }
          }
          break;
        }
        case "unmuted":
          setNotice("您已被解除禁言。");
          break;
        case "room_deleted":
          setNotice("该房间已被删除。");
          setTimeout(() => (location.href = "/"), 1500);
          break;
        case "room_muted": {
          const muted = !!e.muted;
          setNotice(muted ? "房主开启了全员禁言。" : "房主关闭了全员禁言。");
          // 禁言开启即清空语音：未获发言权的用户（含房主）强制退出。
          if (muted && voiceRef.current.inVoice && !voiceInfoRef.current.granted) {
            voiceRef.current.leave();
            setError("房间已开启全员禁言，您已被移出语音。");
          }
          void loadRoom();
          break;
        }
        case "speak_grant": {
          const uid = e.user_id as string;
          if (uid === myUserId) {
            setVoiceInfo((prev) => ({ ...prev, granted: true, pending: false, can_speak: true }));
            setNotice("您已获得发言权限，可以加入语音。");
          } else {
            void loadRoom();
          }
          break;
        }
        case "speak_revoked": {
          const uid = e.user_id as string;
          if (uid === myUserId) {
            setVoiceInfo((prev) => ({ ...prev, granted: false, can_speak: false }));
            setNotice("您的发言权限已被取消。");
            if (voiceRef.current.inVoice) {
              voiceRef.current.leave();
              setError("发言权限已被取消，已退出语音。");
            }
          } else {
            void loadRoom();
          }
          break;
        }
        case "speak_requested":
          setVoiceInfo((prev) => ({ ...prev, pending: true }));
          setNotice("发言申请已提交，等待房主批准。");
          break;
        case "speak_rejected":
          setVoiceInfo((prev) => ({ ...prev, pending: false }));
          setNotice("发言申请被拒绝。");
          break;
        case "speak_request_notify":
          setNotice("有成员申请发言。");
          void loadRoom();
          break;
        case "reaction": {
          const mid = e.message_id as string;
          const reactions = e.reactions as MessageReaction[] | undefined;
          setMessages((prev) =>
            prev.map((m) => (m.id === mid && reactions ? { ...m, reactions } : m)),
          );
          break;
        }
        case "signal": {
          const from = e.from as string;
          const data = e.data as SignalData;
          signalRef.current(from, data);
          break;
        }
        case "media_state": {
          const uid = e.user_id as string;
          if (!uid) break;
          setMediaStates((prev) => ({
            ...prev,
            [uid]: { mic: !!e.mic_muted, speaker: !!e.speaker_muted },
          }));
          break;
        }
        case "error": {
          const msg = (e as { message?: string }).message || "未知错误";
          if ((e as { code?: string }).code === "sfu_required") {
            setError(msg);
          } else {
            setError(msg);
          }
          break;
        }
        default:
          break;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [myUserId],
  );

  const socket = useRoomSocket(code, myUserId, onSocketEvent);
  const voice = useVoiceSession({
    code,
    mode: room?.media_mode ?? null,
    myUserId,
    socket,
    audioBitrate: room?.audio_bitrate ?? 0,
    onSpeakingChange: setSpeaking,
  });
  signalRef.current = voice.handleSignal;
  voiceMicMutedRef.current = voice.micMuted;
  prevModeRef.current = room?.media_mode ?? null;
  // onSocketEvent 是稳定回调，语音/禁言状态经 ref 读取最新值。
  const voiceRef = useRef(voice);
  voiceRef.current = voice;
  const voiceInfoRef = useRef(voiceInfo);
  voiceInfoRef.current = voiceInfo;

  // 把自己的麦克风/扬声器静音状态同步给房间其他成员。
  // 节流：疯狂点静音时合并为最新状态一次广播，避免刷屏。
  const mediaStateRef = useRef({ mic: false, speaker: false });
  const sendMediaState = useRef(
    throttleTrailing(() => {
      const s = mediaStateRef.current;
      socket.send({ type: "media_state", mic_muted: s.mic, speaker_muted: s.speaker });
    }, 300),
  ).current;
  useEffect(() => {
    if (!voice.inVoice || !myUserId) return;
    const state = { mic: voice.micMuted, speaker: voice.speakerMuted };
    mediaStateRef.current = state;
    setMediaStates((prev) => ({ ...prev, [myUserId]: state }));
    sendMediaState();
  }, [voice.inVoice, voice.micMuted, voice.speakerMuted, myUserId, socket, sendMediaState]);

  // 加入语音（用户主动点击）：1 秒节流防连点。状态经 ref 读取避免闭包过期。
  const roomMutedRef = useRef(room?.room_muted ?? false);
  roomMutedRef.current = room?.room_muted ?? false;
  const canSpeakRef = useRef(voiceInfo.can_speak);
  canSpeakRef.current = voiceInfo.can_speak;
  const grantedRef = useRef(voiceInfo.granted);
  grantedRef.current = voiceInfo.granted;
  const joinVoiceInner = useCallback(async () => {
    setError(null);
    // 全员禁言且未获发言权：先申请发言，不直接进语音。
    if (roomMutedRef.current && !canSpeakRef.current && !grantedRef.current) {
      socket.send({ type: "speak_request" });
      return;
    }
    try {
      // 房间加载后 media_mode 才会确定；必须始终调用当前会话的 join，
      // 不能在首轮 room=null 时缓存成 P2P join。
      await voiceRef.current.join();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [socket]);
  // 节流包装只创建一次，但通过 ref 调用最新一次渲染的 joinVoiceInner，
  // 避免 useRef 缓存首轮闭包。
  const joinVoiceInnerRef = useRef(joinVoiceInner);
  joinVoiceInnerRef.current = joinVoiceInner;
  const joinVoice = useRef(
    throttleLeading(() => void joinVoiceInnerRef.current(), 1000),
  ).current;
  // 麦克风测试（本地回放，不占服务器）。声音设置在个人设置模态框修改，
  // 通过订阅自动重建测试链路（useMicTest 内部处理）。
  const micTest = useMicTest();
  const [micPanelOpen, setMicPanelOpen] = useState(false);
  const [pttMode, setPttMode] = useState(false);
  // 离开语音：1 秒节流防连点。同样必须调用当前会话的 leave。
  const leaveVoice = useRef(
    throttleLeading(() => voiceRef.current.leave(), 1000),
  ).current;

  // PTT：空格按住说话、松开静音；输入框中不拦截空格。
  const pttActiveRef = useRef(false);
  useEffect(() => {
    if (!pttMode || !voice.inVoice) return;
    voice.applyMicMuted(true);
    const isTyping = (t: EventTarget | null) =>
      t instanceof HTMLElement &&
      (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
    const down = (e: KeyboardEvent) => {
      if (e.code !== "Space" || isTyping(e.target)) return;
      e.preventDefault();
      if (!pttActiveRef.current) {
        pttActiveRef.current = true;
        voice.applyMicMuted(false);
      }
    };
    const up = (e: KeyboardEvent) => {
      if (e.code !== "Space") return;
      if (pttActiveRef.current) {
        pttActiveRef.current = false;
        voice.applyMicMuted(true);
      }
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      pttActiveRef.current = false;
      voice.applyMicMuted(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pttMode, voice.inVoice]);

  // 入会/离会提示音：语音成员增减时播放（含自己加入/离开）。
  useEffect(() => {
    const prev = prevVoiceRef.current;
    const cur = socket.voiceMembers;
    const joined = cur.filter((u) => !prev.includes(u));
    const left = prev.filter((u) => !cur.includes(u));
    if (joined.length > 0) playChime("join");
    if (left.length > 0) playChime("leave");
    prevVoiceRef.current = cur;
  }, [socket.voiceMembers]);

  const copyRoomLink = () => {
    void navigator.clipboard
      .writeText(location.origin + "/" + code)
      .then(() => {
        setLinkCopied(true);
        setTimeout(() => setLinkCopied(false), 1500);
      })
      .catch(() => undefined);
  };

  // -------------------------------------------------------------- chat

  // @提及建议：按昵称前缀匹配（最多 6 个）。
  const mentionMatches = mentionQuery
    ? members
        .filter((m) => m.user_id !== myUserId)
        .map((m) => ({ user_id: m.user_id, nickname: m.nickname, username: m.username }))
        .filter((m) => (m.nickname || m.username || "").toLowerCase().startsWith(mentionQuery))
        .slice(0, 6)
    : [];

  const send = (e: React.FormEvent) => {
    e.preventDefault();
    const body = input.trim();
    if (!body) return;
    if (
      socket.send({
        type: "send_message",
        body,
        ...(replyingTo ? { reply_to: replyingTo.id } : {}),
      })
    ) {
      setInput("");
      setMentionQuery("");
      setReplyingTo(null);
    } else {
      // 连接断开：保留输入内容，提示重连后重试。
      setError("消息发送失败：连接已断开，重连后请再次发送。");
    }
  };

  // -------------------------------------------------------------- actions

  const setNickname = async (uid: string, nickname: string) => {
    try {
      await api(`/api/v1/rooms/${code}/members/${uid}/nickname`, {
        method: "POST",
        body: JSON.stringify({ nickname }),
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const muteMember = async (uid: string, seconds: number) => {
    try {
      await api(`/api/v1/rooms/${code}/mutes`, {
        method: "POST",
        body: JSON.stringify({ scope: "member", user_id: uid, duration_seconds: seconds }),
      });
      await loadRoom();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const unmute = async (muteId: string) => {
    try {
      await api(`/api/v1/rooms/${code}/mutes/${muteId}`, { method: "DELETE" });
      await loadRoom();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const removeMember = async (uid: string) => {
    try {
      await api(`/api/v1/rooms/${code}/members/${uid}/remove`, { method: "POST", body: "{}" });
      await loadRoom();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const changeRole = async (uid: string, role: string) => {
    try {
      await api(`/api/v1/rooms/${code}/members/${uid}/role`, {
        method: "POST",
        body: JSON.stringify({ role }),
      });
      await loadRoom();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const leaveRoom = async () => {
    try {
      await api(`/api/v1/rooms/${code}/leave`, { method: "POST", body: "{}" });
      location.href = "/";
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  // --------------------------------------------------------------- render

  if (notMember) {
    return (
      <div className="page center">
        <div className="card">
          <h1>房间 /{code}</h1>
          <p className="hint">输入邀请码即可加入。</p>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void join(joinToken);
            }}
          >
            <input
              value={joinToken}
              onChange={(e) => setJoinToken(e.target.value)}
              placeholder="邀请令牌"
            />
            <button disabled={joinBusy || !joinToken}>{joinBusy ? "加入中…" : "加入房间"}</button>
          </form>
          {error && <p className="error">{error}</p>}
        </div>
      </div>
    );
  }

  if (!room) {
    return (
      <div className="page center">
        <div className="card">
          <h1>房间 /{code}</h1>
          {error ? <p className="error">{error}</p> : <p className="hint">加载中…</p>}
          <Link to="/">返回首页</Link>
        </div>
      </div>
    );
  }

  const isOwner = room.my_role === "owner";
  const canManageInvites = caps.includes("manage_invites");
  const canManageMembers = caps.includes("manage_members") || isOwner;
  const canMute = caps.includes("mute_member");
  const canManageSettings = isOwner;
  const myMember = members.find((m) => m.user_id === myUserId);

  return (
    <div className="room-layout">
      <div className="room-main">
        <div className="room-head">
          <h1>
            {room.kind === "direct" ? "私聊房间" : "房间"} /{room.code}
            <span className="tag">{room.state === "active" ? "正常" : room.state === "deleted" ? "已删除" : "已过期"}</span>
            <span className="tag">{room.media_mode === "p2p" ? "P2P" : "SFU"}</span>
            {!room.text_enabled && <span className="tag warn">文字已关闭</span>}
            {room.room_muted && <span className="tag warn">全员禁言中</span>}
            {!socket.connected && <span className="tag warn">重新连接中…</span>}
            {voice.inVoice && voice.quality && (
              <span className={`tag ${voice.quality === "good" ? "ok" : voice.quality === "medium" ? "" : "warn"}`}>
                网络：{voice.quality === "good" ? "优" : voice.quality === "medium" ? "中" : "差"}
              </span>
            )}
          </h1>
          <span className="head-actions">
            <button
              className={roomMutedNotif ? "muted-notif" : ""}
              title={roomMutedNotif ? "已静音本房间通知" : "静音本房间通知"}
              onClick={toggleRoomMutedNotif}
            >
              {roomMutedNotif ? "通知已静音" : "静音通知"}
            </button>
            <button onClick={() => void copyRoomLink()}>
              {linkCopied ? "已复制" : "复制链接"}
            </button>
            <button onClick={leaveRoom}>离开房间</button>
          </span>
          {notice && <p className="ok">{notice}</p>}
          {error && <p className="error">{error}</p>}
        </div>

        <div className="voice-bar">
          {voice.inVoice ? (
            <>
              <span className="tag ok">
                语音中（{voice.participantCount} 人）
              </span>
              {pttMode ? (
                <>
                  <button
                    className="ptt-hold"
                    onPointerDown={() => voice.applyMicMuted(false)}
                    onPointerUp={() => voice.applyMicMuted(true)}
                    onPointerLeave={() => voice.applyMicMuted(true)}
                  >
                    按住说话
                  </button>
                  <button onClick={() => setPttMode(false)}>退出按住说话</button>
                </>
              ) : (
                <>
                  <button onClick={voice.toggleMicrophone}>
                    {voice.micMuted ? "取消静音" : "静音麦克风"}
                  </button>
                  <button onClick={voice.toggleSpeaker}>
                    {voice.speakerMuted ? "取消扬声器静音" : "静音扬声器"}
                  </button>
                  <button onClick={() => setPttMode(true)}>按住说话</button>
                </>
              )}
              <button onClick={leaveVoice}>离开语音</button>
            </>
          ) : (
            <button
              disabled={voice.status === "connecting" || (room?.room_muted && voiceInfo.pending)}
              onClick={() => void joinVoice()}
            >
              {voice.status === "connecting"
                ? "加入语音中…"
                : room?.room_muted && !voiceInfo.can_speak && !voiceInfo.granted
                  ? voiceInfo.pending
                    ? "已申请，等待房主批准"
                    : "申请发言"
                  : "加入语音"}
            </button>
          )}
          <button
            className={micPanelOpen ? "mic-testing" : ""}
            onClick={() => {
              if (micPanelOpen) {
                setMicPanelOpen(false);
                micTest.discard();
              } else {
                setMicPanelOpen(true);
              }
            }}
          >
            测试麦克风
          </button>
          <span style={{ flex: 1 }} />
          <div ref={voice.audioContainerRef} style={{ display: "none" }} />
        </div>

        {micPanelOpen && (
          <div className="mic-test-panel">
            {micTest.error && <p className="error">{micTest.error}</p>}
            {micTest.state === "idle" && (
              <>
                <span>录制 10-30 秒说话内容，对比原声与降噪效果</span>
                <button onClick={micTest.startRecording}>开始录制</button>
              </>
            )}
            {micTest.state === "recording" && (
              <>
                <span className="rec-dot" />
                <span>录制中 {micTest.seconds}s / {60}s…</span>
                <button onClick={micTest.stopRecording}>停止录制</button>
              </>
            )}
            {micTest.state === "recorded" && (
              <>
                <span>已录制 {micTest.seconds}s</span>
                <button
                  className={micTest.playMode === "raw" ? "active" : ""}
                  onClick={() => micTest.play("raw")}
                >
                  播放原声
                </button>
                <button
                  className={micTest.playMode === "processed" ? "active" : ""}
                  disabled={!micTest.recordedMode || micTest.recordedMode === "off"}
                  title={
                    micTest.recordedMode === "off"
                      ? "当前未启用降噪，请先在个人设置选择降噪模式"
                      : micTest.recordedMode === "rnnoise"
                        ? "rnnoise 降噪效果"
                        : "WebRTC 降噪效果"
                  }
                  onClick={() => micTest.play("processed")}
                >
                  播放降噪效果（{micTest.recordedMode === "rnnoise" ? "rnnoise" : micTest.recordedMode === "webrtc" ? "WebRTC" : "未启用"}）
                </button>
                <button onClick={micTest.startRecording}>重新录制</button>
              </>
            )}
            {micTest.state === "playing" && (
              <button onClick={micTest.stopPlay}>停止播放</button>
            )}
          </div>
        )}

        <div className="chat-pane">
          {room.announcement && (
            <div className="announcement">
              <b>公告</b> {room.announcement}
            </div>
          )}
          {room.room_muted && (
            <div className="mute-banner">
              全员禁言模式：仅获发言权用户可语音和发言
            </div>
          )}
          <ul className="messages">
            {messages.map((m) =>
              !m.sender ? (
                <li key={m.id} className="message system">
                  <span className="who system">系统</span>
                  <span className="body">{m.body}</span>
                </li>
              ) : (
                <li key={m.id} className="message">
                  {m.reply && (
                    <span className="reply-quote">
                      <b>回复 {m.reply.sender}</b>：{m.reply.body}
                    </span>
                  )}
                  <span className="who">
                    {m.sender.nickname || m.sender.username || m.sender.user_id?.slice(0, 8)}
                    <span className="msg-time">{fmtMsgTime(m.created_at)}</span>
                  </span>
                  <span className="body">{renderBody(m.body, members)}</span>
                  <span className="msg-actions">
                    <button
                      className="copy-msg"
                      onClick={() => {
                        setReplyingTo(m);
                        document.querySelector<HTMLInputElement>(".composer input")?.focus();
                      }}
                    >
                      回复
                    </button>
                    <button
                      className="copy-msg"
                      onClick={() => {
                        void navigator.clipboard.writeText(m.body).catch(() => undefined);
                      }}
                    >
                      复制
                    </button>
                    <span className="reaction-picker">
                      {REACT_EMOJIS.map((re) => (
                        <button
                          key={re}
                          onClick={() => socket.send({ type: "reaction", message_id: m.id, emoji: re })}
                        >
                          {re}
                        </button>
                      ))}
                    </span>
                  </span>
                  {(m.reactions?.length ?? 0) > 0 && (
                    <span className="reaction-row">
                      {m.reactions!.map((r) => (
                        <button
                          key={r.emoji}
                          className={r.me ? "me" : ""}
                          onClick={() =>
                            socket.send({ type: "reaction", message_id: m.id, emoji: r.emoji })
                          }
                        >
                          {r.emoji} {r.count}
                        </button>
                      ))}
                    </span>
                  )}
                </li>
              ),
            )}
          </ul>
          <form className="composer" onSubmit={send}>
            {replyingTo && (
              <div className="reply-bar">
                回复 {replyingTo.sender?.nickname || replyingTo.sender?.username || "消息"}：
                {replyingTo.body.slice(0, 40)}
                <button
                  className="link"
                  onClick={() => setReplyingTo(null)}
                  aria-label="取消回复"
                >
                  ×
                </button>
              </div>
            )}
            {mentionMatches.length > 0 && (
              <div className="mention-suggest">
                {mentionMatches.map((mm) => (
                  <button
                    key={mm.user_id}
                    onClick={() => {
                      const nm = mm.nickname || mm.username || "";
                      setInput((v) => v.replace(/@[\w\u4e00-\u9fa5]*$/, `@${nm} `));
                      setMentionQuery("");
                    }}
                  >
                    {mm.nickname || mm.username}
                  </button>
                ))}
              </div>
            )}
            <input
              value={input}
              onChange={(e) => {
                const v = e.target.value;
                setInput(v);
                // @提及补全：光标位于 @xxx 词尾时列出匹配成员。
                const at = v.lastIndexOf("@");
                if (at >= 0 && !v.slice(at).includes(" ")) {
                  setMentionQuery(v.slice(at + 1).toLowerCase());
                } else {
                  setMentionQuery("");
                }
              }}
              onKeyDown={(e) => {
                if (e.ctrlKey && e.key === "Enter") {
                  e.preventDefault();
                  send(e);
                }
              }}
              placeholder={
                room.room_muted && !voiceInfo.granted
                  ? "全员禁言中，仅获发言权用户可发言"
                  : room.text_enabled
                    ? "输入消息…"
                    : "房主已关闭文字频道"
              }
              disabled={
                !room.text_enabled || !caps.includes("send_text") || (room.room_muted && !voiceInfo.granted)
              }
            />
            <button
              disabled={
                !room.text_enabled || !caps.includes("send_text") || (room.room_muted && !voiceInfo.granted)
              }
            >
              发送
            </button>
          </form>
        </div>
      </div>

      <aside className="room-side">
        <RoomControls
          room={room}
          code={code}
          caps={caps}
          isOwner={isOwner}
          canManageInvites={canManageInvites}
          canManageSettings={canManageSettings}
          canMute={canMute}
          canManageMembers={canManageMembers}
          members={members}
          mutes={mutes}
          myMember={myMember}
          speaking={speaking}
          mediaStates={mediaStates}
          voiceInfo={voiceInfo}
          socket={socket}
          myRole={me?.user.role || "guest"}
          isGuest={me?.user.role === "guest"}
          onNickname={setNickname}
          onMute={muteMember}
          onUnmute={unmute}
          onRemove={removeMember}
          onRole={changeRole}
          onRemoteVolume={voice.setRemoteVolume}
          onChanged={loadRoom}
          onError={setError}
        />
      </aside>
    </div>
  );
}

interface ControlsProps {
  room: Room;
  code: string;
  caps: string[];
  isOwner: boolean;
  canManageInvites: boolean;
  canManageSettings: boolean;
  canMute: boolean;
  canManageMembers: boolean;
  members: Member[];
  mutes: Mute[];
  myMember?: Member;
  speaking: Record<string, number>;
  mediaStates: Record<string, { mic: boolean; speaker: boolean }>;
  voiceInfo: RoomVoice;
  socket: RoomSocket;
  myRole: string;
  isGuest: boolean;
  onNickname: (uid: string, nick: string) => void;
  onMute: (uid: string, seconds: number) => void;
  onUnmute: (id: string) => void;
  onRemove: (uid: string) => void;
  onRole: (uid: string, role: string) => void;
  onRemoteVolume: (uid: string, v: number) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}

const REACT_EMOJIS = ["👍", "❤️", "😂", "😮", "😢", "😡", "🎉"];

/** @昵称 高亮渲染（成员昵称匹配，词边界）。 */
function highlightMentions(text: string, members: Member[]): React.ReactNode {
  const names = members
    .map((m) => m.nickname || m.username || "")
    .filter((n) => n.length > 0);
  if (names.length === 0) return text;
  const esc = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`@(${names.map(esc).join("|")})(?![\\w-])`, "g");
  const out: React.ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) out.push(text.slice(last, m.index));
    out.push(
      <span className="mention" key={m.index}>
        @{m[1]}
      </span>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out.length > 0 ? out : text;
}

/** 桌面通知：仅页面不可见且用户已开启通知时弹出。 */
function pushNotify(title: string, body: string): void {
  if (!document.hidden || typeof Notification === "undefined" || Notification.permission !== "granted") {
    return;
  }
  try {
    if (localStorage.getItem("cloudroom.notify") === "1") {
      new Notification(title, { body });
    }
  } catch {
    /* 忽略通知失败 */
  }
}

/** 消息时间戳：今天显示 HH:MM，今年 M月D日 HH:MM，更早含年份。 */
function fmtMsgTime(iso: string): string {
  const d = new Date(iso);
  const now = new Date();
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  if (d.toDateString() === now.toDateString()) return `${hh}:${mm}`;
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日 ${hh}:${mm}`;
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日 ${hh}:${mm}`;
}

/** 消息正文渲染：URL 转链接 + @提及高亮。 */
function renderBody(text: string, members: Member[]): React.ReactNode {
  const urlRe = /(https?:\/\/[^\s<>"'）)】]+)/g;
  const parts: React.ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = urlRe.exec(text)) !== null) {
    if (m.index > last) parts.push(highlightMentions(text.slice(last, m.index), members));
    const url = m[1];
    parts.push(
      <a key={m.index} href={url} target="_blank" rel="noopener noreferrer" className="msg-link">
        {url}
      </a>,
    );
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push(highlightMentions(text.slice(last), members));
  return parts.length > 0 ? parts : text;
}

/** 最后上线时间的相对文案。 */
function fmtAgo(iso: string, now: number): string {
  const diff = Math.max(0, now - new Date(iso).getTime());
  const m = Math.floor(diff / 60000);
  if (m < 1) return "刚刚";
  if (m < 60) return `${m} 分钟前`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} 小时前`;
  return `${Math.floor(h / 24)} 天前`;
}

const AVATAR_COLORS = ["#4c8dff", "#2ea46a", "#d9a13b", "#b46bd9", "#e06c75", "#3fb6c8"];

function avatarColor(id: string): string {
  let h = 0;
  for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}

function RoomControls(p: ControlsProps) {
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [inviteExpiry, setInviteExpiry] = useState("24h");
  const [volOpen, setVolOpen] = useState<string | null>(null);
  const [volValues, setVolValues] = useState<Record<string, number>>({});
  const [memberQuery, setMemberQuery] = useState("");
  // 在线状态判定时钟：每分钟刷新"最后上线"文案。
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 60000);
    return () => window.clearInterval(t);
  }, []);
  const [settings, setSettings] = useState({
    text_enabled: p.room.text_enabled,
    allow_direct_messages: p.room.allow_direct_messages,
    audio_bitrate: p.room.audio_bitrate || 48,
    announcement: p.room.announcement || "",
    max_members: p.room.max_members,
    expires_at: "",
  });
  const [nickEdits, setNickEdits] = useState<Record<string, string>>({});
  const [modeTarget, setModeTarget] = useState<"p2p" | "sfu">(
    p.room.media_mode === "p2p" ? "sfu" : "p2p",
  );
  const [modeBitrate, setModeBitrate] = useState(p.room.audio_bitrate || 48);
  const [modeReqNote, setModeReqNote] = useState<string | null>(null);
  const [modeReqBusy, setModeReqBusy] = useState(false);
  const [bitrateTiers, setBitrateTiers] = useState<
    { kbps: number; min_owner_role: string }[]
  >([]);

  // 码率档位（含房主角色门槛），按当前账号角色过滤可用项。
  useEffect(() => {
    api<{ bitrate_tiers: { kbps: number; min_owner_role: string }[] }>("/api/v1/voice/options")
      .then((r) => setBitrateTiers(r.bitrate_tiers || []))
      .catch(() => undefined);
  }, []);
  const roleRank = (role: string): number =>
    role === "super_admin" ? 3 : role === "admin" ? 2 : role === "user" ? 1 : 0;
  const availableTiers = bitrateTiers.filter(
    (t) => roleRank(p.myRole) >= roleRank(t.min_owner_role),
  );

  useEffect(() => {
    setSettings((s) => ({
      ...s,
      text_enabled: p.room.text_enabled,
      allow_direct_messages: p.room.allow_direct_messages,
      audio_bitrate: p.room.audio_bitrate || 48,
      announcement: p.room.announcement || "",
      max_members: p.room.max_members,
    }));
  }, [p.room.text_enabled, p.room.allow_direct_messages, p.room.audio_bitrate, p.room.announcement, p.room.max_members]);

  useEffect(() => {
    setModeTarget(p.room.media_mode === "p2p" ? "sfu" : "p2p");
    setModeReqNote(null);
  }, [p.room.media_mode]);

  const createInvite = async () => {
    try {
      const r = await api<{ invite_url: string }>(`/api/v1/rooms/${p.code}/invites`, {
        method: "POST",
        body: JSON.stringify({ expires_in: inviteExpiry }),
      });
      setInviteUrl(r.invite_url);
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    }
  };

  const saveSettings = async () => {
    try {
      await api(`/api/v1/rooms/${p.code}/settings`, {
        method: "POST",
        body: JSON.stringify({
          text_enabled: settings.text_enabled,
          allow_direct_messages: settings.allow_direct_messages,
          // P2P 房间不设码率（避免残留高档位在切回 SFU 前被误校验）。
          audio_bitrate: p.room.media_mode === "sfu" ? settings.audio_bitrate : 0,
          announcement: settings.announcement,
          max_members: settings.max_members,
          expires_at: settings.expires_at ? new Date(settings.expires_at).toISOString() : null,
        }),
      });
      await p.onChanged();
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    }
  };

  const deleteRoom = async () => {
    if (!window.confirm("删除该房间？房间号将永久作废。")) return;
    try {
      await api(`/api/v1/rooms/${p.code}/delete`, { method: "POST", body: "{}" });
      location.href = "/";
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    }
  };

  // 媒体模式变更与建房走同一条申请/审批通道（/api/v1/room-requests）。
  const requestModeChange = async () => {
    setModeReqBusy(true);
    setModeReqNote(null);
    try {
      const r = await api<{ request: RoomRequest; auto_approved?: boolean }>(
        "/api/v1/room-requests",
        {
          method: "POST",
          body: JSON.stringify({
            kind: "change_media_mode",
            room_code: p.code,
            media_mode: modeTarget,
            // 切到 SFU 时携带目标码率（切到 P2P 由后端强制 0）。
            audio_bitrate: modeTarget === "sfu" ? modeBitrate : 0,
            note: "房主申请变更媒体模式",
          }),
        },
      );
      if (r.auto_approved) {
        setModeReqNote("已生效。");
        await p.onChanged();
      } else {
        setModeReqNote("申请已提交，等待管理员审批。");
      }
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    } finally {
      setModeReqBusy(false);
    }
  };

  // 私聊入口：点头像发起/打开与该成员的私聊（仅标准房间且房主开启私聊时）。
  const startDirect = useCallback(
    async (peerId: string) => {
      if (!peerId || peerId === p.myMember?.user_id) return;
      try {
        const r = await api<{ room: Room }>(`/api/v1/rooms/${p.code}/direct`, {
          method: "POST",
          body: JSON.stringify({ user_id: peerId }),
        });
        location.href = `/${r.room.code}`;
      } catch (e) {
        p.onError(e instanceof Error ? e.message : String(e));
      }
    },
    [p.code, p.myMember?.user_id, p.onError],
  );
  const dmBusyRef = useRef(false);
  const startDirectThrottled = useCallback(
    (peerId: string) => {
      if (dmBusyRef.current) return;
      dmBusyRef.current = true;
      setTimeout(() => {
        dmBusyRef.current = false;
      }, 800);
      void startDirect(peerId);
    },
    [startDirect],
  );
  // 全员禁言开关与审批动作：600ms 节流防连点（后端亦有幂等兜底）。
  const roomMutedRef = useRef(p.room.room_muted);
  roomMutedRef.current = p.room.room_muted;
  const toggleMute = useRef(
    throttleLeading(() => p.socket.send({ type: "room_mute_toggle", muted: !roomMutedRef.current }), 600),
  ).current;
  const approveSpeak = useRef(
    throttleLeading((uid: string) => p.socket.send({ type: "speak_reply", user_id: uid, approve: true }), 600),
  ).current;
  const rejectSpeak = useRef(
    throttleLeading((uid: string) => p.socket.send({ type: "speak_reply", user_id: uid, approve: false }), 600),
  ).current;
  const revokeSpeak = useRef(
    throttleLeading((uid: string) => p.socket.send({ type: "speak_revoke", user_id: uid }), 600),
  ).current;
  const relinquishSpeak = useRef(
    throttleLeading(() => p.socket.send({ type: "speak_relinquish" }), 600),
  ).current;

  return (
    <div className="side-stack">
      <section className="card">
        <h2>
          成员（{p.members.length}
          {p.room.max_members > 0 ? `/${p.room.max_members}` : ""}）
        </h2>
        <input
          className="member-search"
          placeholder="搜索成员"
          value={memberQuery}
          onChange={(e) => setMemberQuery(e.target.value)}
        />
        {/* 禁言模式：获发言权用户独立列表（含自己放弃 / 房主取消） */}
        {p.room.room_muted && (
          <div className="speakers">
            <h3>已获发言权</h3>
            <ul>
              {(p.voiceInfo.grants ?? []).map((g) => (
                <li key={g.user_id}>
                  {g.nickname}
                  {g.user_id === p.myMember?.user_id ? "（我）" : ""}
                  {g.user_id === p.myMember?.user_id && (
                    <button onClick={relinquishSpeak}>放弃发言权限</button>
                  )}
                  {p.isOwner && g.user_id !== p.myMember?.user_id && (
                    <button onClick={() => revokeSpeak(g.user_id)}>取消</button>
                  )}
                </li>
              ))}
              {(p.voiceInfo.grants ?? []).length === 0 && <li className="hint">暂无</li>}
            </ul>
          </div>
        )}
        <ul className="members">
          {/* 自己置顶，其余按原顺序；支持昵称搜索过滤 */}
          {[...p.members]
            .filter((m) => {
              if (!memberQuery) return true;
              const q = memberQuery.toLowerCase();
              return (m.nickname || m.username || "").toLowerCase().includes(q);
            })
            .sort((a, b) => {
              if (a.user_id === p.myMember?.user_id) return -1;
              if (b.user_id === p.myMember?.user_id) return 1;
              return 0;
            })
            .map((m) => {
            // 全员禁言中未获发言权的用户即使有麦克风输入也不显示说话动效。
            const level =
              p.room.room_muted && !p.voiceInfo.granted && m.user_id === p.myMember?.user_id
                ? 0
                : p.speaking[m.user_id] || 0;
            const st = p.mediaStates[m.user_id];
            const canDm =
              p.room.kind === "standard" &&
              p.room.allow_direct_messages &&
              m.user_id !== p.myMember?.user_id;
            // 在线 = 最近 90 秒内有心跳（presence 心跳 60s）。
            const isOnline = !!m.last_online_at && now - new Date(m.last_online_at).getTime() < 90000;
            return (
            <li key={m.user_id} className={m.user_id === p.myMember?.user_id ? "me" : ""}>
              <span className="member-main">
                <span
                  className={`presence ${isOnline ? "online" : "offline"}`}
                  title={isOnline ? "在线" : `最后上线 ${m.last_online_at ? fmtAgo(m.last_online_at, now) : "未知"}`}
                />
                <button
                  className={`avatar-mini ${level > 0 ? "speaking" : ""}`}
                  style={
                    m.avatar_url
                      ? ({
                          background: `url(${m.avatar_url}) center/cover no-repeat`,
                          "--lv": String(Math.min(1, level * 1.6)),
                        } as React.CSSProperties)
                      : ({
                          background: avatarColor(m.user_id),
                          "--lv": String(Math.min(1, level * 1.6)),
                        } as React.CSSProperties)
                  }
                  title={canDm ? `与 ${m.nickname || m.username || m.user_id.slice(0, 8)} 私聊` : undefined}
                  disabled={!canDm}
                  onClick={() => void startDirectThrottled(m.user_id)}
                >
                  {m.avatar_url ? "" : (m.nickname || m.username || m.user_id || "?")[0].toUpperCase()}
                </button>
                <span className="who">
                  {m.nickname || m.username || m.user_id.slice(0, 8)}
                  <span className="tag">{m.room_role === "owner" ? "房主" : "成员"}</span>
                </span>
              </span>
              {m.user_id !== p.myMember?.user_id && (
                <button
                  className="vol-btn"
                  title="调节该用户音量"
                  onClick={() =>
                    setVolOpen((uid) => (uid === m.user_id ? null : m.user_id))
                  }
                >
                  {volOpen === m.user_id ? "收起" : "音量"}
                </button>
              )}
              {canDm && (
                <button className="dm-btn" onClick={() => void startDirectThrottled(m.user_id)}>
                  私聊
                </button>
              )}
              {st?.mic && <span className="tag mute-tag">麦克风静音</span>}
              {st?.speaker && <span className="tag mute-tag">扬声器静音</span>}
              {volOpen === m.user_id && (
                <span className="vol-slider">
                  <input
                    type="range"
                    min={0}
                    max={200}
                    step={10}
                    value={volValues[m.user_id] ?? Math.round(getVolumePref(m.user_id) * 100)}
                    onChange={(e) => {
                      const v = Number(e.target.value) / 100;
                      setVolValues((prev) => ({ ...prev, [m.user_id]: v * 100 }));
                      p.onRemoteVolume(m.user_id, v);
                    }}
                  />
                  <span>
                    {Math.round(volValues[m.user_id] ?? getVolumePref(m.user_id) * 100)}%
                  </span>
                </span>
              )}
              {m.user_id === p.myMember?.user_id && (
                <span className="row-actions">
                  <input
                    placeholder="修改我的昵称"
                    value={nickEdits[m.user_id] ?? ""}
                    onChange={(e) => setNickEdits((x) => ({ ...x, [m.user_id]: e.target.value }))}
                  />
                  <button
                    onClick={() => {
                      const nick = nickEdits[m.user_id];
                      if (nick) void p.onNickname(m.user_id, nick);
                    }}
                  >
                    改昵称
                  </button>
                </span>
              )}
              {(p.isOwner || p.caps.includes("manage_members") || p.caps.includes("manage_nicknames")) &&
                m.user_id !== p.myMember?.user_id &&
                m.room_role !== "owner" && (
                  <span className="row-actions">
                    <input
                      placeholder="昵称"
                      value={nickEdits[m.user_id] ?? ""}
                      onChange={(e) => setNickEdits((x) => ({ ...x, [m.user_id]: e.target.value }))}
                    />
                    <button
                      onClick={() => {
                        const nick = nickEdits[m.user_id];
                        if (nick) void p.onNickname(m.user_id, nick);
                      }}
                    >
                      重命名
                    </button>
                    {p.canMute && (
                      <button onClick={() => void p.onMute(m.user_id, 3600)}>禁言 1 小时</button>
                    )}
                    {p.canManageMembers && (
                      <button onClick={() => void p.onRemove(m.user_id)}>移出</button>
                    )}
                  </span>
                )}
            </li>
            );
          })}
        </ul>
        {p.mutes.length > 0 && (
          <div>
            <h3>当前禁言</h3>
            <ul>
              {p.mutes.map((m) => (
                <li key={m.mute_id}>
                  {m.scope === "room" ? "全员" : `用户 ${m.target_user_id?.slice(0, 8)}`}
                  <button onClick={() => void p.onUnmute(m.mute_id)}>解除禁言</button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </section>

      {p.canManageInvites && (
        <section className="card">
          <h2>邀请</h2>
          <div className="row-actions">
            <select value={inviteExpiry} onChange={(e) => setInviteExpiry(e.target.value)}>
              <option value="24h">有效期 24 小时</option>
              <option value="7d">有效期 7 天</option>
              <option value="30d">有效期 30 天</option>
              <option value="1h">有效期 1 小时</option>
            </select>
            <button onClick={() => void createInvite()}>生成邀请链接</button>
          </div>
          {inviteUrl && (
            <p className="invite-url">
              <code>{location.origin + inviteUrl}</code>
              <button
                onClick={() => {
                  void navigator.clipboard.writeText(location.origin + inviteUrl);
                }}
              >
                复制
              </button>
            </p>
          )}
        </section>
      )}

      {p.isOwner && p.room.kind === "standard" && (
        <section className="card">
          <h2>媒体模式变更</h2>
          <p className="hint">变更需管理员审批。</p>
          <div className="row-actions">
            <select
              value={modeTarget}
              onChange={(e) => setModeTarget(e.target.value as "p2p" | "sfu")}
            >
              <option value="p2p">P2P</option>
              <option value="sfu">SFU</option>
            </select>
            {modeTarget === "sfu" && (
              <select
                value={modeBitrate}
                onChange={(e) => setModeBitrate(Number(e.target.value))}
              >
                {availableTiers.map((t) => (
                  <option key={t.kbps} value={t.kbps}>
                    {t.kbps} kbps
                  </option>
                ))}
              </select>
            )}
            <button disabled={modeReqBusy || modeTarget === p.room.media_mode} onClick={() => void requestModeChange()}>
              {modeReqBusy ? "提交中…" : "申请变更"}
            </button>
          </div>
          {modeReqNote && <p className="ok">{modeReqNote}</p>}
        </section>
      )}

      {p.isOwner && (
        <section className="card">
          <h2>房间设置</h2>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.text_enabled}
              onChange={(e) => setSettings((s) => ({ ...s, text_enabled: e.target.checked }))}
            />
            文字频道开启
          </label>
          {p.room.kind === "standard" && (
            <label className="check">
              <input
                type="checkbox"
                checked={settings.allow_direct_messages}
                onChange={(e) =>
                  setSettings((s) => ({ ...s, allow_direct_messages: e.target.checked }))
                }
              />
              允许私聊
            </label>
          )}
          {p.room.kind === "standard" && (
            <label>
              过期时间（留空 = 永久保留）
              <input
                type="datetime-local"
                value={settings.expires_at}
                onChange={(e) => setSettings((s) => ({ ...s, expires_at: e.target.value }))}
              />
            </label>
          )}
          <label>
            房间公告（空 = 不显示）
            <textarea
              value={settings.announcement}
              onChange={(e) => setSettings((s) => ({ ...s, announcement: e.target.value }))}
              maxLength={500}
              rows={2}
            />
          </label>
          <label>
            成员上限（0 = 不限）
            <input
              type="number"
              min={0}
              max={500}
              value={settings.max_members}
              onChange={(e) =>
                setSettings((s) => ({ ...s, max_members: Math.max(0, Number(e.target.value) || 0) }))
              }
            />
          </label>
          {p.room.media_mode === "sfu" && (
            <label>
              音频码率
              <select
                value={settings.audio_bitrate}
                onChange={(e) =>
                  setSettings((s) => ({ ...s, audio_bitrate: Number(e.target.value) }))
                }
              >
                {availableTiers.map((t) => (
                  <option key={t.kbps} value={t.kbps}>
                    {t.kbps} kbps
                  </option>
                ))}
              </select>
            </label>
          )}
          <button onClick={() => void saveSettings()}>保存设置</button>
          <button className="danger" onClick={() => void deleteRoom()}>
            删除房间
          </button>
        </section>
      )}

      {p.isOwner && p.room.kind === "standard" && (
        <section className="card">
          <h2>全员禁言</h2>
          <p className="hint">开启后仅获准发言者可语音和发言。</p>
          <div className="row-actions">
            <button onClick={toggleMute}>
              {p.room.room_muted ? "关闭全员禁言" : "开启全员禁言"}
            </button>
          </div>
          {p.room.room_muted && (
            <>
              <h3>发言申请</h3>
              <ul>
                {(p.voiceInfo.requests ?? []).map((r) => (
                  <li key={r.user_id}>
                    {r.nickname}
                    <button onClick={() => approveSpeak(r.user_id)}>批准</button>
                    <button onClick={() => rejectSpeak(r.user_id)}>拒绝</button>
                  </li>
                ))}
                {(p.voiceInfo.requests ?? []).length === 0 && <li className="hint">暂无申请</li>}
              </ul>
            </>
          )}
        </section>
      )}

      {p.isGuest && (
        <section className="card">
          <h2>访客账号</h2>
          <p className="hint">注册并绑定后，房间成员关系将保留到新账号。</p>
          <GuestUpgrade onError={p.onError} />
        </section>
      )}
    </div>
  );
}

function GuestUpgrade({ onError }: { onError: (m: string) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [invite, setInvite] = useState("");
  const [busy, setBusy] = useState(false);
  const doUpgrade = async () => {
    if (password !== confirm) {
      onError("两次输入的密码不一致");
      return;
    }
    setBusy(true);
    try {
      await api<{ user: import("../api").User; bound_rooms: string[] }>(
        "/api/v1/auth/upgrade-guest",
        { method: "POST", body: JSON.stringify({ username, password, invite_token: invite }) },
      );
      // 等 Set-Cookie 落盘后再整页刷新：会话已切换为新账号，
      // 刷新后房间成员关系（已转移）让用户直接以正式身份回到房间。
      const { promise, resolve } = Promise.withResolvers<void>();
      setTimeout(resolve, 300);
      await promise;
      location.reload();
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div>
      <input placeholder="用户名" value={username} onChange={(e) => setUsername(e.target.value)} />
      <input
        type="password"
        placeholder="密码"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      <input
        type="password"
        placeholder="确认密码"
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
      />
      <input placeholder="邀请码" value={invite} onChange={(e) => setInvite(e.target.value)} />
      <button
        disabled={busy || !username || !password || !confirm || !invite}
        onClick={() => void doUpgrade()}
      >
        {busy ? "注册中…" : "注册并绑定"}
      </button>
    </div>
  );
}


