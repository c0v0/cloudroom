import { serverNow, setServerNow } from "../lib/clock";
import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { api, apiOrigin, assetUrl, Attachment, Member, Message, MessageReaction, Mute, NotificationPrefs, Room, RoomRequest, RoomVoice } from "../api";
import { useAuth } from "../auth";
import { useRoomSocket } from "../hooks/useRoomSocket";
import type { RoomSocket, RoomSocketEvent } from "../hooks/useRoomSocket";
import { useVoiceSession } from "../hooks/useVoiceSession";
import type { SignalData } from "../hooks/useP2PVoice";
import { playChime, playMessageChime } from "../hooks/chime";
import { throttleLeading, throttleTrailing } from "../hooks/throttle";
import { getVolumePref } from "../hooks/remoteAudio";
import { MessageList } from "../components/MessageList";
import { Webhooks } from "../components/Webhooks";
import { mergeMessages, mergeOlder } from "../lib/messages";
import { fmtAgo } from "../lib/time";
import { soundEnabled, subscribeSound } from "../lib/soundPref";
import { loadBitrateTiers } from "../lib/voiceOptions";
import { setSpeakingLevels, useSpeakingLevels } from "../lib/speakingStore";

export default function RoomPage() {
  const { code = "" } = useParams();
  const [params] = useSearchParams();
  const hashToken = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const inviteToken = params.get("invite") || hashToken.get("invite");
  const { me, refresh } = useAuth();
  const coarsePointer = typeof window !== "undefined" && !!window.matchMedia?.("(pointer: coarse)").matches;

  const [room, setRoom] = useState<Room | null>(null);
  const [members, setMembers] = useState<Member[]>([]);
  const [mutes, setMutes] = useState<Mute[]>([]);
  const [caps, setCaps] = useState<string[]>([]);
  const [messages, setMessages] = useState<Message[]>([]);
  const [pins, setPins] = useState<Message[]>([]);
  const [emojis, setEmojis] = useState<Record<string, string>>({});
  const [hasMore, setHasMore] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  // 未读胶囊、附件草稿、搜索面板
  const [unread, setUnread] = useState(0);
  const [pendingAttachment, setPendingAttachment] = useState<Attachment | null>(null);
  const [uploading, setUploading] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<Message[] | null>(null);
  const [bookmarkedIds, setBookmarkedIds] = useState<Set<string>>(new Set());
  const [searchBusy, setSearchBusy] = useState(false);
  // 公告关闭状态：按房间记忆，公告文本变化后重新出现。
  const [announcementDismissed, setAnnouncementDismissed] = useState(false);
  const messagesRef = useRef<Message[]>([]);
  messagesRef.current = messages;
  // 消息列表滚动：贴底时跟随新消息，翻页时保持当前视口锚点。
  const listRef = useRef<HTMLUListElement | null>(null);
  const stickToBottomRef = useRef(true);
  const scrollAnchorRef = useRef<{ height: number; top: number } | null>(null);
  const onMessagesScroll = useCallback(() => {
    const el = listRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
    stickToBottomRef.current = atBottom;
    if (atBottom) setUnread(0);
  }, []);
  const [input, setInput] = useState("");
  const [mentionQuery, setMentionQuery] = useState("");
  const [replyingTo, setReplyingTo] = useState<Message | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dmInvite, setDmInvite] = useState<{ code: string; fromName: string } | null>(null);
  const [notMember, setNotMember] = useState(false);
  const [joinToken, setJoinToken] = useState(inviteToken || "");
  const [joinBusy, setJoinBusy] = useState(false);
  const [linkCopied, setLinkCopied] = useState(false);
  const [mediaStates, setMediaStates] = useState<
    Record<string, { mic: boolean; speaker: boolean }>
  >({});
  // 已读游标（user_id → 最大已读 seq）、举手、正在输入、语音表情飘屏。
  const [reads, setReads] = useState<Record<string, number>>({});
  const [hands, setHands] = useState<string[]>([]);
  const [typing, setTyping] = useState<Record<string, { name: string; until: number }>>({});
  const [voiceReactions, setVoiceReactions] = useState<
    { id: string; emoji: string; name: string }[]
  >([]);
  const [voiceInfo, setVoiceInfo] = useState<RoomVoice>({
    can_speak: true,
    granted: false,
    pending: false,
  });
  const prevVoiceRef = useRef<string[]>([]);

  const lastSeqRef = useRef(0);
  const readSentRef = useRef(0);
  const myUserId = me?.user.user_id;
  const membersRef = useRef<Member[]>([]);
  membersRef.current = members;
  // 新消息提示音开关（个人设置）：模块缓存 + 订阅，避免每次渲染读 localStorage。
  const soundEnabledRef = useRef(soundEnabled());
  useEffect(() => subscribeSound((v) => { soundEnabledRef.current = v; }), []);
  const notifyPrefsRef = useRef<NotificationPrefs>({ mode: "mentions", dnd_until: null });
  useEffect(() => {
    const onPrefs = (e: Event) => {
      const p = (e as CustomEvent<NotificationPrefs>).detail;
      if (p) notifyPrefsRef.current = p;
    };
    window.addEventListener("cloudroom:notify-prefs", onPrefs);
    return () => window.removeEventListener("cloudroom:notify-prefs", onPrefs);
  }, []);
  useEffect(() => {
    let cancelled = false;
    void api<NotificationPrefs>("/api/v1/me/notification-prefs")
      .then((p) => {
        if (!cancelled && p) notifyPrefsRef.current = p;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  // 房间静音（服务端按用户+房间保存，跨设备同步；localStorage 仅作即时缓存）。
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
  useEffect(() => {
    let cancelled = false;
    void api<{ muted: boolean }>(`/api/v1/rooms/${code}/notification-prefs`)
      .then((r) => {
        if (cancelled) return;
        setRoomMutedNotif(r.muted);
        roomMutedPrefRef.current = r.muted;
        try {
          const all = JSON.parse(localStorage.getItem("cloudroom.mutedRooms") || "{}") as Record<string, boolean>;
          all[code] = r.muted;
          localStorage.setItem("cloudroom.mutedRooms", JSON.stringify(all));
        } catch {
          /* 忽略 */
        }
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [code]);
  const toggleRoomMutedNotif = async () => {
    const next = !roomMutedNotif;
    setRoomMutedNotif(next);
    roomMutedPrefRef.current = next;
    try {
      await api(`/api/v1/rooms/${code}/notification-prefs`, {
        method: "PUT",
        body: JSON.stringify({ muted: next }),
      });
      const all = JSON.parse(localStorage.getItem("cloudroom.mutedRooms") || "{}") as Record<string, boolean>;
      all[code] = next;
      localStorage.setItem("cloudroom.mutedRooms", JSON.stringify(all));
    } catch (e) {
      setRoomMutedNotif(!next);
      roomMutedPrefRef.current = !next;
      setError(e instanceof Error ? e.message : String(e));
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

  const loadPins = useCallback(async () => {
    try {
      const r = await api<{ messages: Message[] }>(`/api/v1/rooms/${code}/pins`);
      setPins(r.messages);
    } catch {
      /* pinned list is non-critical */
    }
  }, [code]);

  useEffect(() => {
    void loadPins();
  }, [loadPins]);

  useEffect(() => {
    api<{ emojis: { name: string; url: string }[] }>("/api/v1/emojis")
      .then((r) => setEmojis(Object.fromEntries(r.emojis.map((e) => [e.name, e.url]))))
      .catch(() => undefined);
  }, []);

  // 渲染后同步滚动位置：翻页用锚点补偿高度差，否则贴底跟随新消息。
  useLayoutEffect(() => {
    const el = listRef.current;
    if (!el) return;
    const anchor = scrollAnchorRef.current;
    if (anchor) {
      el.scrollTop = el.scrollHeight - anchor.height + anchor.top;
      scrollAnchorRef.current = null;
      return;
    }
    if (stickToBottomRef.current) el.scrollTop = el.scrollHeight;
  }, [messages]);

  // 公告关闭状态：同一房间同一文本只关闭一次，公告更新后重新出现。
  useEffect(() => {
    if (!room?.announcement) {
      setAnnouncementDismissed(false);
      return;
    }
    try {
      setAnnouncementDismissed(
        localStorage.getItem(`cloudroom.announcement.${code}`) === room.announcement,
      );
    } catch {
      setAnnouncementDismissed(false);
    }
  }, [code, room?.announcement]);

  // 首屏取最新一页（服务端尾部窗口），更早的消息用「加载更早」按 before 游标翻页。
  const loadMessages = useCallback(async () => {
    try {
      const r = await api<{ messages: Message[]; last_seq: number }>(
        `/api/v1/rooms/${code}/messages?limit=50`,
      );
      let list = r.messages;
      const sp = new URLSearchParams(window.location.search);
      const jumpId = sp.get("msg");
      const jumpSeq = Number(sp.get("seq") || 0);
      if (jumpId && jumpSeq > 0 && !list.some((m) => m.id === jumpId)) {
        try {
          const target = await api<{ messages: Message[] }>(
            `/api/v1/rooms/${code}/messages?after=${jumpSeq - 1}&limit=1`,
          );
          list = mergeMessages(list, target.messages);
        } catch {
          /* 目标消息不可见时保留当前窗口 */
        }
      }
      setMessages(list);
      setHasMore(r.messages.length >= 50);
      lastSeqRef.current = Math.max(lastSeqRef.current, r.last_seq);
      if (jumpId) {
        window.setTimeout(() => {
          document
            .getElementById(`msg-${jumpId}`)
            ?.scrollIntoView({ block: "center" });
        }, 0);
      }
    } catch {
      /* room gone */
    }
  }, [code]);
  const loadOlder = useCallback(async () => {
    const first = messagesRef.current[0]?.seq;
    if (!first) return;
    setLoadingOlder(true);
    const el = listRef.current;
    if (el) scrollAnchorRef.current = { height: el.scrollHeight, top: el.scrollTop };
    try {
      const r = await api<{ messages: Message[] }>(
        `/api/v1/rooms/${code}/messages?before=${first}&limit=200`,
      );
      setMessages((prev) => mergeOlder(r.messages, prev));
      setHasMore(r.messages.length >= 200);
    } catch {
      /* 翻页失败：保留当前窗口 */
    } finally {
      setLoadingOlder(false);
    }
  }, [code]);
  // 断线重连后补齐缺口：hello.last_seq 大于本地游标时按 after 增量拉取。
  // 单页上限 200，必须循环到追平，否则长断线会留下永久空洞。
  const backfill = useCallback(
    async (fromSeq: number, targetSeq: number) => {
      let cursor = fromSeq;
      for (let page = 0; page < 20 && cursor < targetSeq; page++) {
        try {
          const r = await api<{ messages: Message[] }>(
            `/api/v1/rooms/${code}/messages?after=${cursor}&limit=200`,
          );
          if (r.messages.length === 0) break;
          setMessages((prev) => mergeMessages(prev, r.messages));
          const max = Math.max(...r.messages.map((m) => m.seq));
          if (max <= cursor) break;
          cursor = max;
          lastSeqRef.current = Math.max(lastSeqRef.current, cursor);
        } catch {
          /* 补齐失败：不打断会话，下次重连再试 */
          break;
        }
      }
    },
    [code],
  );

  // 切换房间（SPA 内 /:code 复用）时先清空上一房间的状态，避免消息/成员/草稿串房。
  useEffect(() => {
    setMessages([]);
    setMembers([]);
    setMutes([]);
    setCaps([]);
    setHasMore(false);
    setUnread(0);
    setReads({});
    setHands([]);
    setTyping({});
    setVoiceReactions([]);
    setReplyingTo(null);
    setPendingAttachment(null);
    setSearchOpen(false);
    setSearchResults(null);
    // 房间级临时状态也必须清空，避免上一个房间的麦克风/发言/通知状态泄漏。
    setMediaStates({});
    setSpeakingLevels({});
    setVoiceInfo({ can_speak: true, granted: false, pending: false });
    try {
      const all = JSON.parse(localStorage.getItem("cloudroom.mutedRooms") || "{}") as Record<
        string,
        boolean
      >;
      setRoomMutedNotif(!!all[code]);
    } catch {
      setRoomMutedNotif(false);
    }
    lastSeqRef.current = 0;
    readSentRef.current = 0;
    setInput(localStorage.getItem(`cloudroom.draft.${code}`) ?? "");
  }, [code]);

  useEffect(() => {
    void loadRoom();
    void loadMessages();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  // 邀请 URL：打开即自动进房（未注册用户同样适用）。
  // 用 ref 防 StrictMode 双执行与手动点击竞态。
  const joinInFlightRef = useRef(false);
  useEffect(() => {
    if (notMember && inviteToken && !joinInFlightRef.current) {
      joinInFlightRef.current = true;
      void join(inviteToken).finally(() => {
        joinInFlightRef.current = false;
      });
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
      if (params.get("invite") || hashToken.get("invite")) history.replaceState(null, "", `/${code}`);
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
  // 用于识别 @提及：账号昵称/用户名 + 本房间昵称（成员可改房间昵称）。
  const myNamesRef = useRef<string[]>([]);
  myNamesRef.current = [
    me?.user.username,
    members.find((m) => m.user_id === myUserId)?.nickname,
  ].filter((n): n is string => !!n && n.length > 0);

  const onSocketEvent = useCallback(
    (e: RoomSocketEvent) => {
      switch (e.type) {
        case "hello": {
          const m = e.members as Member[];
          if (Array.isArray(m)) setMembers(m);
          // 重连后刷新房间快照（设置/公告/禁言可能已变）。
          // 只在 hello 确认自己已是成员时刷新，避免访客加入前 403 把界面打回邀请页。
          if (Array.isArray(m) && m.some((x) => x.user_id === myUserId)) {
            void loadRoom();
          }
          const helloReads = e.reads as Record<string, number> | undefined;
          if (helloReads && typeof helloReads === "object") setReads(helloReads);
          const helloHands = e.hands as string[] | undefined;
          setServerNow(e.now as string | undefined);
          if (Array.isArray(helloHands)) setHands(helloHands);
          const helloSeq = (e as { last_seq?: number }).last_seq || 0;
          const prevSeq = lastSeqRef.current;
          if (prevSeq > 0 && helloSeq > prevSeq) void backfill(prevSeq, helloSeq);
          lastSeqRef.current = Math.max(prevSeq, helloSeq);
          break;
        }
        case "typing": {
          const uid = e.user_id as string;
          if (!uid || uid === myUserId) break;
          const name = (e.nickname as string) || "成员";
          const ttl = Number(e.ttl_ms) || 4000;
          setTyping((prev) => ({ ...prev, [uid]: { name, until: Date.now() + ttl } }));
          break;
        }
        case "read": {
          const uid = e.user_id as string;
          const seq = Number(e.seq) || 0;
          if (!uid) break;
          setReads((prev) => ((prev[uid] ?? 0) >= seq ? prev : { ...prev, [uid]: seq }));
          break;
        }
        case "hand": {
          const list = e.hands as string[] | undefined;
          if (Array.isArray(list)) setHands(list);
          break;
        }
        case "voice_reaction": {
          const emoji = String(e.emoji || "");
          if (!emoji) break;
          const id = `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
          const name = (e.nickname as string) || "成员";
          setVoiceReactions((prev) => [...prev.slice(-5), { id, emoji, name }]);
          window.setTimeout(() => {
            setVoiceReactions((prev) => prev.filter((r) => r.id !== id));
          }, 2600);
          break;
        }
        case "message": {
          const m = (e as unknown as { message: Message }).message;
          setMessages((prev) => mergeMessages(prev, [m]));
          // 不在底部时累计未读，配合「N 条新消息」胶囊。
          if (!stickToBottomRef.current && m.sender?.user_id !== myUserId) {
            setUnread((n) => n + 1);
          }
          lastSeqRef.current = Math.max(lastSeqRef.current, m.seq);
          const from = m.sender?.nickname || m.sender?.username || m.sender?.user_id?.slice(0, 8) || "";
          const muted = roomMutedPrefRef.current;
          // 提及：匹配 @账号昵称/@用户名/@房间昵称（与正文高亮规则一致）。
          const mentioned =
            !!from &&
            myNamesRef.current.some((n) => m.body.includes(`@${n}`));
          const replied = !!(m.reply?.uid && m.reply.uid === myUserId);
          const prefs = notifyPrefsRef.current;
          const quiet =
            prefs.mode === "none" ||
            (!!prefs.dnd_until && Date.parse(prefs.dnd_until) > Date.now());
          const allowSpecial = !quiet;
          const allowAll = !quiet && prefs.mode === "all" && !muted;
          // 桌面通知：按个人通知范围过滤；回复/提及优先，房间静音只抑制普通消息。
          if (from) {
            if (replied && allowSpecial) {
              pushNotify(
                `房间 /${code}`,
                `${from} 回复了你：${m.body.slice(0, 60)}`,
              );
            } else if (mentioned && allowSpecial) {
              pushNotify(
                `房间 /${code}`,
                `${from} 提到了你：${m.body.slice(0, 60)}`,
              );
            } else if (allowAll) {
              pushNotify(
                `房间 /${code}`,
                `${from} ${m.body.slice(0, 60)}`,
              );
            }
          }
          // 新消息提示音：与桌面通知同一范围；房间静音时仅回复/提及响。
          if (from && (allowAll || (allowSpecial && (replied || mentioned))) && soundEnabledRef.current) {
            playMessageChime();
          }
          break;
        }
        case "message_pinned": {
          const id = e.message_id as string;
          if (id) setMessages((prev) => prev.map((m) => (m.id === id ? { ...m, pinned: true } : m)));
          void loadPins();
          break;
        }
        case "message_unpinned": {
          const id = e.message_id as string;
          if (id)
            setMessages((prev) =>
              prev.map((m) => (m.id === id ? { ...m, pinned: false } : m)),
            );
          void loadPins();
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
        case "owner_transferred":
          void loadRoom();
          break;
        case "room_settings": {
          const r = e.room as Room;
          const modeChanged =
            !!prevModeRef.current && !!r.media_mode && r.media_mode !== prevModeRef.current;
          setRoom((prev) => (prev ? { ...prev, ...r } : prev));
          if (modeChanged) {
            // 旧模式的信令/媒体在新模式下无效：先退出语音，避免继续发 p2p 信令被服务端拒绝。
            voiceRef.current.leave();
            setNotice("房间媒体模式已变更，请重新加入语音。");
          }
          break;
        }
        case "muted": {
          if (e.target_user_id === myUserId) {
            setNotice("您已被管理员禁言。");
            voiceMicMutedRef.current = true;
          }
          // 禁言列表与 send_text 能力都在房间快照里，必须刷新而不是只改本地 ref。
          void loadRoom();
          break;
        }
        case "unmuted": {
          const target = e.user_id as string | undefined;
          if (!target || target === myUserId) {
            setNotice("您已被解除禁言。");
          }
          void loadRoom();
          break;
        }
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
            // 只更新本地 grants，避免每个成员都发一次 GET /rooms（大房间扇出）。
            const nickname =
              membersRef.current.find((m) => m.user_id === uid)?.nickname || "";
            setVoiceInfo((prev) => {
              const grants = prev.grants ?? [];
              if (grants.some((g) => g.user_id === uid)) return prev;
              return {
                ...prev,
                grants: [
                  ...grants,
                  { user_id: uid, nickname, granted_at: new Date().toISOString() },
                ],
              };
            });
          }
          break;
        }
        case "speak_revoked": {
          const uid = e.user_id as string;
          // 主动放弃（relinquished）不是被取消：不提示、不退出语音。
          const relinquished = e.relinquished === true;
          if (uid === myUserId) {
            setVoiceInfo((prev) => ({ ...prev, granted: false, can_speak: false }));
            if (!relinquished) {
              setNotice("您的发言权限已被取消。");
              if (voiceRef.current.inVoice) {
                voiceRef.current.leave();
                setError("发言权限已被取消，已退出语音。");
              }
            }
          } else {
            setVoiceInfo((prev) => ({
              ...prev,
              grants: (prev.grants ?? []).filter((g) => g.user_id !== uid),
            }));
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
        case "message_updated": {
          const m = (e as unknown as { message: Message }).message;
          setMessages((prev) => prev.map((x) => (x.id === m.id ? { ...x, ...m } : x)));
          break;
        }
        case "message_deleted": {
          const mid = e.message_id as string;
          const m = (e as unknown as { message?: Message }).message;
          setMessages((prev) =>
            prev.map((x) => {
              if (x.id !== mid) return x;
              const sender =
                x.sender && m?.sender ? { ...x.sender, ...m.sender } : x.sender;
              return { ...x, ...(m ?? {}), deleted: true, body: "", sender };
            }),
          );
          break;
        }
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
        case "direct_room_created": {
          const r = e.room as Room | undefined;
          const peerUserId = e.peer_user_id as string | undefined;
          // 只让被私聊的一方看到提示；发起方已经在 startDirect 里跳转。
          if (!r || peerUserId !== myUserId) break;
          const from = r.owner_user_id
            ? membersRef.current.find((m) => m.user_id === r.owner_user_id)
            : undefined;
          const fromName = from?.nickname || from?.username || "对方";
          setDmInvite({ code: r.code, fromName });
          const prefs = notifyPrefsRef.current;
          const quiet =
            prefs.mode === "none" ||
            (!!prefs.dnd_until && Date.parse(prefs.dnd_until) > Date.now());
          if (soundEnabledRef.current && !roomMutedPrefRef.current && !quiet) {
            playMessageChime();
          }
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
    [myUserId, code],
  );

  const socket = useRoomSocket(code, myUserId, onSocketEvent);

  // 正在输入过期清理（1s 检查，避免每个事件都挂定时器）。
  useEffect(() => {
    const timer = window.setInterval(() => {
      setTyping((prev) => {
        const now = Date.now();
        const entries = Object.entries(prev).filter(([, v]) => v.until > now);
        if (entries.length === Object.keys(prev).length) return prev;
        return Object.fromEntries(entries);
      });
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  // 已读回执：只在页面可见且停在底部时上报，节流到 1s。
  useEffect(() => {
    if (!socket.connected || document.hidden) return;
    if (!stickToBottomRef.current) return;
    const seq = lastSeqRef.current;
    if (seq <= readSentRef.current) return;
    const timer = window.setTimeout(() => {
      readSentRef.current = seq;
      socket.send({ type: "read", seq });
    }, 400);
    return () => window.clearTimeout(timer);
  }, [socket, messages.length]);

  // 输入中：节流 2.5s，避免每次按键都广播。
  const lastTypingRef = useRef(0);
  const notifyTyping = useCallback(() => {
    const now = Date.now();
    if (now - lastTypingRef.current < 2500) return;
    lastTypingRef.current = now;
    socket.send({ type: "typing" });
  }, [socket]);

  const voice = useVoiceSession({
    code,
    mode: room?.media_mode ?? null,
    myUserId,
    socket,
    audioBitrate: room?.audio_bitrate ?? 0,
    e2ee: room?.e2ee ?? false,
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
  }, [socket]);
  // 节流包装只创建一次，但通过 ref 调用最新一次渲染的 joinVoiceInner，
  // 避免 useRef 缓存首轮闭包。
  const joinVoiceInnerRef = useRef(joinVoiceInner);
  joinVoiceInnerRef.current = joinVoiceInner;
  const joinVoice = useRef(
    throttleLeading(() => void joinVoiceInnerRef.current(), 1000),
  ).current;
  // 麦克风测试（本地回放，不占服务器）。声音设置在个人设置模态框修改，
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

  const send = (e: { preventDefault: () => void }) => {
    e.preventDefault();
    const body = input.trim();
    if (!body && !pendingAttachment) return;
    if (
      socket.send({
        type: "send_message",
        body,
        ...(replyingTo ? { reply_to: replyingTo.id } : {}),
        ...(pendingAttachment ? { attachment_id: pendingAttachment.id } : {}),
      })
    ) {
      setInput("");
      try {
        localStorage.removeItem(`cloudroom.draft.${code}`);
      } catch {
        /* ignore */
      }
      setMentionQuery("");
      setReplyingTo(null);
      setPendingAttachment(null);
    } else {
      // 连接断开：保留输入内容，提示重连后重试。
      setError("消息发送失败：连接已断开，重连后请再次发送。");
    }
  };

  // -------------------------------------------------------------- actions

  const replyToMessage = useCallback((m: Message) => {
    setReplyingTo(m);
    // 附件按钮的 file input 也在 .composer 内，必须只聚焦文本输入框。
    document
      .querySelector<HTMLInputElement>('.composer input[type="text"], .composer input:not([type])')
      ?.focus();
  }, []);

  const copyMessage = useCallback((body: string) => {
    void navigator.clipboard.writeText(body).catch(() => undefined);
  }, []);

  const reactToMessage = useCallback(
    (messageId: string, emoji: string) => {
      socket.send({ type: "reaction", message_id: messageId, emoji });
    },
    [socket],
  );

  const editMessage = useCallback(
    async (messageId: string, body: string) => {
      try {
        await api(`/api/v1/rooms/${code}/messages/${messageId}/edit`, {
          method: "POST",
          body: JSON.stringify({ body }),
        });
        return true;
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        return false;
      }
    },
    [code],
  );

  const deleteMessage = useCallback(
    async (messageId: string) => {
      try {
        await api(`/api/v1/rooms/${code}/messages/${messageId}/delete`, {
          method: "POST",
          body: "{}",
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [code],
  );

  useEffect(() => {
    let cancelled = false;
    void api<{ message_ids: string[] }>(`/api/v1/rooms/${code}/bookmarks`)
      .then((r) => {
        if (!cancelled) setBookmarkedIds(new Set(r.message_ids));
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [code]);

  const toggleBookmark = useCallback(
    async (m: Message) => {
      const on = bookmarkedIds.has(m.id);
      try {
        await api(`/api/v1/rooms/${code}/messages/${m.id}/bookmark`, {
          method: on ? "DELETE" : "POST",
        });
        setBookmarkedIds((prev) => {
          const next = new Set(prev);
          if (on) next.delete(m.id);
          else next.add(m.id);
          return next;
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [bookmarkedIds, code],
  );

  const togglePin = useCallback(
    async (m: Message) => {
      try {
        await api(
          `/api/v1/rooms/${code}/messages/${m.id}/${m.pinned ? "unpin" : "pin"}`,
          { method: "POST", body: "{}" },
        );
        await loadPins();
        await loadRoom();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [code, loadPins, loadRoom],
  );

  const uploadAttachment = useCallback(
    async (file: File) => {
      setUploading(true);
      setError(null);
      try {
        const form = new FormData();
        form.append("file", file);
        const r = await api<{ attachment: Attachment }>(
          `/api/v1/rooms/${code}/attachments`,
          { method: "POST", body: form },
        );
        setPendingAttachment(r.attachment);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setUploading(false);
      }
    },
    [code],
  );

  const runSearch = useCallback(async () => {
    const q = searchQuery.trim();
    if (!q) return;
    setSearchBusy(true);
    try {
      const r = await api<{ messages: Message[] }>(
        `/api/v1/rooms/${code}/messages/search?q=${encodeURIComponent(q)}&limit=30`,
      );
      setSearchResults(r.messages);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSearchResults([]);
    } finally {
      setSearchBusy(false);
    }
  }, [code, searchQuery]);

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

  const muteMember = async (uid: string, seconds: number | null) => {
    try {
      await api(`/api/v1/rooms/${code}/mutes`, {
        method: "POST",
        body: JSON.stringify({ scope: "member", user_id: uid, duration_seconds: seconds ?? undefined }),
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
  const myMember = members.find((m) => m.user_id === myUserId);

  return (
    <div className="room-layout">
      <div className="voice-reaction-layer" aria-hidden="true">
        {voiceReactions.map((r) => (
          <span key={r.id} className="voice-reaction">
            {r.emoji}
            <small>{r.name}</small>
          </span>
        ))}
      </div>
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
            <span className="head-direct">
              <button
                className={roomMutedNotif ? "muted-notif" : ""}
                title={roomMutedNotif ? "已静音本房间通知" : "静音本房间通知"}
                onClick={toggleRoomMutedNotif}
              >
                {roomMutedNotif ? "通知已静音" : "静音通知"}
              </button>
              <button onClick={() => setSearchOpen((v) => !v)}>搜索</button>
              <button onClick={() => void copyRoomLink()}>
                {linkCopied ? "已复制" : "复制链接"}
              </button>
            </span>
            {!(isOwner && room.kind === "standard") && (
              <button onClick={leaveRoom}>离开房间</button>
            )}
            <details className="head-more">
              <summary>更多</summary>
              <span className="head-more-menu">
                <button
                  className={roomMutedNotif ? "muted-notif" : ""}
                  onClick={toggleRoomMutedNotif}
                >
                  {roomMutedNotif ? "通知已静音" : "静音通知"}
                </button>
                <button onClick={() => setSearchOpen((v) => !v)}>搜索</button>
                <button onClick={() => void copyRoomLink()}>
                  {linkCopied ? "已复制" : "复制链接"}
                </button>
              </span>
            </details>
          </span>
          {notice && <p className="ok">{notice}</p>}
          {error && <p className="error">{error}</p>}
        </div>

        {dmInvite && (
          <div className="dm-invite">
            <span>{dmInvite.fromName} 发起了私聊</span>
            <button onClick={() => (location.href = `/${dmInvite.code}`)}>
              进入私聊
            </button>
            <button onClick={() => setDmInvite(null)}>忽略</button>
          </div>
        )}

        <div className="voice-bar">
          {voice.inVoice ? (
            <>
              <span className="tag ok">
                语音中（{voice.participantCount} 人）
              </span>
              <button
                className={hands.includes(myUserId || "") ? "active" : ""}
                onClick={() =>
                  socket.send({
                    type: "hand",
                    raised: !hands.includes(myUserId || ""),
                  })
                }
                title="举手/放下手"
              >
                {hands.includes(myUserId || "") ? "放下手 ✋" : "举手"}
              </button>
              <span className="voice-emoji-bar" aria-label="发送表情">
                {["👍", "👏", "🎉", "😂", "❤️", "🔥"].map((emoji) => (
                  <button
                    key={emoji}
                    className="emoji-btn"
                    aria-label={`发送表情 ${emoji}`}
                    onClick={() => socket.send({ type: "voice_reaction", emoji })}
                  >
                    {emoji}
                  </button>
                ))}
              </span>
              {voice.aecStatus === "unsupported" && (
                <span className="tag warn" title="当前浏览器或设备未启用回声消除，外放时可能听到回声；建议使用耳机。">
                  回声消除不可用
                </span>
              )}
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
              {room?.media_mode === "sfu" && (
                <button
                  className="screen-share-btn"
                  onClick={() => {
                    void voice.toggleScreenShare().catch((e: unknown) => {
                      setError(e instanceof Error ? e.message : "屏幕共享失败。");
                    });
                  }}
                >
                  {voice.screenSharing ? "停止共享" : "共享屏幕"}
                </button>
              )}
              <button
                className={`leave-voice${room?.media_mode === "sfu" ? "" : " leave-voice-wide"}`}
                onClick={leaveVoice}
              >
                离开语音
              </button>
            </>
          ) : (
            <button
              className="join-voice"
              disabled={voice.status === "connecting" || (room?.room_muted && voiceInfo.pending)}
              onClick={() => void joinVoice()}
            >
              {voice.status === "connecting"
                ? "加入语音中…"
                : room?.room_muted && !voiceInfo.can_speak && !voiceInfo.granted
                  ? voiceInfo.pending
                    ? "等待批准"
                    : "申请发言"
                  : "加入语音"}
            </button>
          )}
          {voice.inVoice && <span className="voice-spacer" />}
          <div ref={voice.audioContainerRef} style={{ display: "none" }} />
        </div>

        <div className="chat-pane">
          <div className="screen-share" ref={voice.screenContainerRef} />
          {room.announcement && !announcementDismissed && (
            <div className="announcement">
              <b>公告</b>
              <span className="announcement-body">
                {room.announcement.split(/(https?:\/\/[^\s<>"'）)】]+)/g).map((part, i) =>
                  /^https?:\/\//.test(part) ? (
                    <a
                      key={i}
                      href={part}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="msg-link"
                    >
                      {part}
                    </a>
                  ) : (
                    <span key={i}>{part}</span>
                  ),
                )}
              </span>
              <button
                className="link announcement-close"
                aria-label="关闭公告"
                title="关闭公告（公告更新后会再次显示）"
                onClick={() => {
                  try {
                    localStorage.setItem(`cloudroom.announcement.${code}`, room.announcement);
                  } catch {
                    /* 隐私模式忽略 */
                  }
                  setAnnouncementDismissed(true);
                }}
              >
                ×
              </button>
            </div>
          )}
          {room.room_muted && (
            <div className="mute-banner">
              全员禁言模式：仅获发言权用户可语音和发言
            </div>
          )}
          {searchOpen && (
            <div className="search-panel">
              <form
                className="search-bar"
                onSubmit={(e) => {
                  e.preventDefault();
                  void runSearch();
                }}
              >
                <input
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  placeholder="搜索本房间消息…"
                  autoFocus
                />
                <button disabled={searchBusy}>{searchBusy ? "搜索中…" : "搜索"}</button>
                <button
                  type="button"
                  onClick={() => {
                    setSearchOpen(false);
                    setSearchResults(null);
                    setSearchQuery("");
                  }}
                >
                  关闭
                </button>
              </form>
              {searchResults && (
                <div className="search-results">
                  {searchResults.length === 0 && <p className="hint">没有匹配的消息。</p>}
                  {searchResults.map((m) => (
                    <button
                      key={m.id}
                      className="search-hit"
                      onClick={() => {
                        setSearchOpen(false);
                        setSearchResults(null);
                        setMessages((prev) => mergeMessages(prev, [m]));
                        requestAnimationFrame(() => {
                          const el = document.getElementById(`msg-${m.id}`);
                          el?.scrollIntoView({ block: "center" });
                        });
                      }}
                    >
                      <span className="who">
                        {m.sender?.nickname || m.sender?.username || "成员"}
                      </span>
                      <span className="body">{m.body.slice(0, 120)}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          )}
          {pins.length > 0 && (
            <div className="pinned-bar">
              <b>固定消息 {pins.length}</b>
              <ul>
                {pins.map((m) => (
                  <li key={m.id}>
                    <button
                      className="link"
                      onClick={() =>
                        document.getElementById(`msg-${m.id}`)?.scrollIntoView({ block: "center" })
                      }
                    >
                      {m.sender?.nickname || m.sender?.username || "成员"}：
                      {m.body.slice(0, 80)}
                    </button>
                    {isOwner && (
                      <button className="link danger" onClick={() => void togglePin(m)}>
                        取消
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
          <ul
            className="messages"
            ref={listRef}
            onScroll={onMessagesScroll}
            role="log"
            aria-live="polite"
            aria-relevant="additions"
            aria-label="消息记录"
          >
            {hasMore && (
              <li className="load-older">
                <button onClick={() => void loadOlder()} disabled={loadingOlder}>
                  {loadingOlder ? "加载中…" : "加载更早的消息"}
                </button>
              </li>
            )}
            <MessageList
              messages={messages}
              members={members}
              onReply={replyToMessage}
              onCopy={copyMessage}
              onReact={reactToMessage}
              onEdit={editMessage}
              onDelete={deleteMessage}
              onPin={isOwner ? togglePin : undefined}
              onBookmark={toggleBookmark}
              bookmarkedIds={bookmarkedIds}
              emojis={emojis}
              myUserId={myUserId}
              reads={reads}
              canModerate={
                isOwner || me?.user.role === "admin" || me?.user.role === "super_admin"
              }
            />
          </ul>
          {unread > 0 && (
            <button
              className="unread-pill"
              onClick={() => {
                const el = listRef.current;
                if (el) el.scrollTop = el.scrollHeight;
                stickToBottomRef.current = true;
                setUnread(0);
              }}
            >
              {unread} 条新消息 ↓
            </button>
          )}
          <form className="composer" onSubmit={send}>
            {(() => {
              const names = Object.values(typing).map((t) => t.name);
              if (names.length === 0) return null;
              const label =
                names.length === 1
                  ? `${names[0]} 正在输入…`
                  : `${names.slice(0, 2).join("、")}${names.length > 2 ? " 等" : ""} 正在输入…`;
              return (
                <div className="typing-indicator" aria-live="polite">
                  {label}
                </div>
              );
            })()}
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
            {pendingAttachment && (
              <div className="attach-chip">
                <span>{pendingAttachment.filename}</span>
                <button
                  type="button"
                  className="link"
                  onClick={() => setPendingAttachment(null)}
                  aria-label="移除附件"
                >
                  ×
                </button>
              </div>
            )}
            {room.attachments_enabled && (
            <label className="file-btn attach-btn" title="添加图片、音频、视频或 PDF">
              {uploading ? "上传中…" : "附件"}
              <input
                type="file"
                accept="image/png,image/jpeg,image/webp,image/gif,audio/mpeg,audio/wav,audio/ogg,audio/mp4,video/mp4,video/webm,application/pdf"
                onChange={(e) => {
                  const f = e.target.files?.[0];
                  if (f) void uploadAttachment(f);
                  e.target.value = "";
                }}
              />
            </label>
            )}
            <textarea
              rows={1}
              value={input}
              onChange={(e) => {
                const v = e.target.value;
                setInput(v);
                try {
                  const key = `cloudroom.draft.${code}`;
                  if (v) localStorage.setItem(key, v);
                  else localStorage.removeItem(key);
                } catch {
                  /* storage may be unavailable */
                }
                if (v) notifyTyping();
                // @提及补全：光标位于 @xxx 词尾时列出匹配成员。
                const at = v.lastIndexOf("@");
                if (at >= 0 && !v.slice(at).includes(" ")) {
                  setMentionQuery(v.slice(at + 1).toLowerCase());
                } else {
                  setMentionQuery("");
                }
              }}
              onKeyDown={(e) => {
                // Enter 发送；Shift+Enter 换行（Markdown 代码块需要多行）。
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  send(e);
                }
              }}
              placeholder={
                room.room_muted && !voiceInfo.granted
                  ? "全员禁言中"
                  : room.text_enabled
                    ? coarsePointer
                      ? "输入消息…"
                      : "输入消息…（Enter 发送，Shift+Enter 换行）"
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
          key={code}
          room={room}
          code={code}
          caps={caps}
          isOwner={isOwner}
          canManageInvites={canManageInvites}
          canMute={canMute}
          canManageMembers={canManageMembers}
          members={members}
          mutes={mutes}
          myMember={myMember}
          mediaStates={mediaStates}
          voiceInfo={voiceInfo}
          hands={hands}
          socket={socket}
          myRole={me?.user.role || "guest"}
          isGuest={me?.user.role === "guest"}
          webhooksEnabled={me?.features?.webhooks === true}
          onNickname={setNickname}
          onMute={muteMember}
          onUnmute={unmute}
          onRemove={removeMember}
          onRemoteVolume={voice.setRemoteVolume}
          onChanged={loadRoom}
          onError={setError}
          onNotice={setNotice}
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
  canMute: boolean;
  canManageMembers: boolean;
  members: Member[];
  mutes: Mute[];
  myMember?: Member;
  mediaStates: Record<string, { mic: boolean; speaker: boolean }>;
  voiceInfo: RoomVoice;
  hands: string[];
  socket: RoomSocket;
  myRole: string;
  isGuest: boolean;
  webhooksEnabled: boolean;
  onNickname: (uid: string, nick: string) => void;
  onMute: (uid: string, seconds: number | null) => void;
  onUnmute: (id: string) => void;
  onRemove: (uid: string) => void;
  onRemoteVolume: (uid: string, v: number) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
  onNotice: (msg: string) => void;
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

const AVATAR_COLORS = ["#4c8dff", "#2ea46a", "#d9a13b", "#b46bd9", "#e06c75", "#3fb6c8"];

function avatarColor(id: string): string {
  let h = 0;
  for (const ch of id) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}

/** 头像 + 说话动效：只有这个叶子组件订阅电平，侧栏其余部分不随音量重渲染。 */
function MemberAvatar({
  userId,
  name,
  avatarUrl,
  online,
  suppressed,
  owner,
}: {
  userId: string;
  name: string;
  avatarUrl?: string | null;
  online: boolean;
  suppressed: boolean;
  owner: boolean;
}) {
  const levels = useSpeakingLevels();
  const level = suppressed ? 0 : levels[userId] || 0;
  const lv = String(Math.min(1, level * 1.6));
  const avatarSrc = assetUrl(avatarUrl ?? null);
  return (
    <span
      className={`avatar-mini ${level > 0 ? "speaking" : ""} ${owner ? "owner" : ""}`}
      aria-label={`${name}，${online ? "在线" : "离线"}${level > 0 ? "，正在说话" : ""}`}
      title={name}
      style={
        avatarSrc
          ? ({ background: `url(${avatarSrc}) center/cover no-repeat`, "--lv": lv } as React.CSSProperties)
          : ({ background: avatarColor(userId), "--lv": lv } as React.CSSProperties)
      }
    >
      {avatarUrl ? "" : (name || "?")[0].toUpperCase()}
    </span>
  );
}

function RoomControls(p: ControlsProps) {
  const [inviteUrl, setInviteUrl] = useState<string | null>(null);
  const [inviteExpiry, setInviteExpiry] = useState("24h");
  const [inviteCopied, setInviteCopied] = useState(false);
  const [volOpen, setVolOpen] = useState<string | null>(null);
  const [volValues, setVolValues] = useState<Record<string, number>>({});
  const [muteSeconds, setMuteSeconds] = useState<Record<string, number>>({});
  const [openMemberMenu, setOpenMemberMenu] = useState<string | null>(null);
  const [editingNick, setEditingNick] = useState(false);
  const [nickDraft, setNickDraft] = useState("");
  const nickCancelRef = useRef(false);
  const [memberQuery, setMemberQuery] = useState("");
  // 与 600ms 节流窗口对齐：给点击一个可见反馈，避免连点被静默丢弃。
  const [muteBusy, setMuteBusy] = useState(false);

  const requestGuestUpgrade = async (uid: string) => {
    try {
      await api(`/api/v1/rooms/${p.code}/guest-upgrade-requests`, {
        method: "POST",
        body: JSON.stringify({ user_id: uid }),
      });
      p.onNotice("已提交转正申请，等待管理员审批。");
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    }
  };
  // 在线状态判定时钟：每分钟刷新"最后上线"文案。
  const [now, setNow] = useState(() => serverNow());
  useEffect(() => {
    const t = window.setInterval(() => setNow(serverNow()), 60000);
    return () => window.clearInterval(t);
  }, []);
  const localInput = (iso: string | null): string => {
    if (!iso) return "";
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "";
    return new Date(d.getTime() - d.getTimezoneOffset() * 60000)
      .toISOString()
      .slice(0, 16);
  };
  const [settings, setSettings] = useState({
    text_enabled: p.room.text_enabled,
    allow_direct_messages: p.room.allow_direct_messages,
    audio_bitrate: p.room.audio_bitrate ?? 48,
    announcement: p.room.announcement || "",
    max_members: p.room.max_members,
    expires_at: localInput(p.room.expires_at),
    e2ee: p.room.e2ee,
    guest_upgrade_open: p.room.guest_upgrade_open ?? false,
  });
  const [nickEdits, setNickEdits] = useState<Record<string, string>>({});
  const [modeTarget, setModeTarget] = useState<"p2p" | "sfu">(
    p.room.media_mode === "p2p" ? "sfu" : "p2p",
  );
  const [modeBitrate, setModeBitrate] = useState(p.room.audio_bitrate ?? 48);
  const [modeReqNote, setModeReqNote] = useState<string | null>(null);
  const [modeReqBusy, setModeReqBusy] = useState(false);
  const [bitrateTiers, setBitrateTiers] = useState<
    { kbps: number; min_owner_role: string }[]
  >([]);

  // 码率档位（含房主角色门槛），按当前账号角色过滤可用项。
  // 只有房主需要这份数据；模块级缓存避免首页与房间重复请求。
  useEffect(() => {
    if (!p.isOwner) return;
    void loadBitrateTiers()
      .then(setBitrateTiers)
      .catch(() => undefined);
  }, [p.isOwner]);
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
      audio_bitrate: p.room.audio_bitrate ?? 48,
      announcement: p.room.announcement || "",
      max_members: p.room.max_members,
      expires_at: localInput(p.room.expires_at),
      e2ee: p.room.e2ee,
      guest_upgrade_open: p.room.guest_upgrade_open ?? false,
    }));
  }, [p.room.text_enabled, p.room.allow_direct_messages, p.room.audio_bitrate, p.room.announcement, p.room.max_members, p.room.expires_at, p.room.e2ee, p.room.guest_upgrade_open]);

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
      setInviteCopied(false);
    } catch (e) {
      p.onError(e instanceof Error ? e.message : String(e));
    }
  };

  const copyInvite = async () => {
    if (!inviteUrl) return;
    try {
      await navigator.clipboard.writeText(location.origin + inviteUrl);
      setInviteCopied(true);
      window.setTimeout(() => setInviteCopied(false), 2000);
    } catch {
      p.onError("复制失败，请手动选择邀请链接。");
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
          e2ee: settings.e2ee,
          guest_upgrade_open: settings.guest_upgrade_open,
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
    [p],
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
      <section className="card members-card">
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
            const isSelf = m.user_id === p.myMember?.user_id;
            // 全员禁言中未获发言权的用户即使有麦克风输入也不显示说话动效。
            const suppressed =
              p.room.room_muted && !p.voiceInfo.granted && isSelf;
            const st = p.mediaStates[m.user_id];
            const canDm =
              p.room.kind === "standard" &&
              p.room.allow_direct_messages &&
              m.user_id !== p.myMember?.user_id;
            const displayName = m.nickname || m.username || m.user_id.slice(0, 8);
            const canUpgradeGuest =
              p.isOwner && m.room_role === "member" && !m.username;
            const canAdmin =
              (p.isOwner || p.caps.includes("manage_members") || p.caps.includes("manage_nicknames")) &&
              m.user_id !== p.myMember?.user_id &&
              m.room_role !== "owner";
            // 在线 = 最近 90 秒内有心跳（presence 心跳 60s）。
            const isOnline = !!m.last_online_at && now - new Date(m.last_online_at).getTime() < 90000;
            return (
            <li key={m.user_id} className={isSelf ? "me" : ""}>
              <div className="member-row">
              <span className="member-main">
                <span
                  className={`presence ${isOnline ? "online" : "offline"}`}
                  aria-hidden="true"
                  title={isOnline ? "在线" : `最后上线 ${m.last_online_at ? fmtAgo(m.last_online_at, now) : "未知"}`}
                />
                <MemberAvatar
                  userId={m.user_id}
                  name={m.nickname || m.username || m.user_id.slice(0, 8)}
                  avatarUrl={m.avatar_url}
                  online={isOnline}
                  suppressed={suppressed}
                  owner={m.room_role === "owner"}
                />
                <span className={`who ${m.room_role === "owner" ? "owner" : ""}`}>
                  {isSelf && editingNick ? (
                    <input
                      className="nick-inline"
                      value={nickDraft}
                      autoFocus
                      maxLength={32}
                      onChange={(e) => setNickDraft(e.target.value)}
                      onBlur={() => {
                        if (nickCancelRef.current) {
                          nickCancelRef.current = false;
                          return;
                        }
                        const nick = nickDraft.trim();
                        if (nick) void p.onNickname(m.user_id, nick);
                        setEditingNick(false);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          e.currentTarget.blur();
                        } else if (e.key === "Escape") {
                          nickCancelRef.current = true;
                          setEditingNick(false);
                        }
                      }}
                    />
                  ) : isSelf ? (
                    <button
                      type="button"
                      className="who-name who-editable"
                      title={`${displayName}（点击修改昵称）`}
                      onClick={() => {
                        setNickDraft(displayName);
                        nickCancelRef.current = false;
                        setEditingNick(true);
                      }}
                    >
                      {displayName}
                    </button>
                  ) : (
                    <span className="who-name" title={displayName}>{displayName}</span>
                  )}
                  {p.hands.includes(m.user_id) && (
                    <span className="hand-badge" title="已举手" aria-label="已举手">
                      ✋
                    </span>
                  )}
                </span>
              </span>
              <span className="member-actions">
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
              {(canAdmin || canUpgradeGuest) && (
                <button
                  className="member-more-btn"
                  onClick={() =>
                    setOpenMemberMenu((prev) => (prev === m.user_id ? null : m.user_id))
                  }
                >
                  {openMemberMenu === m.user_id ? "收起" : "管理"}
                </button>
              )}
              </span>
              </div>
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
              {(canAdmin || canUpgradeGuest) && openMemberMenu === m.user_id && (
                <div className="member-menu-body member-menu-panel">
                    {canUpgradeGuest && (
                      <button
                        className="link"
                        title="为该访客申请转正，管理员批准后可直接注册"
                        onClick={() => void requestGuestUpgrade(m.user_id)}
                      >
                        申请转正
                      </button>
                    )}
                    {canAdmin && (
                      <>
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
                          <>
                            <select
                              aria-label="禁言时长"
                              value={muteSeconds[m.user_id] ?? 3600}
                              onChange={(e) =>
                                setMuteSeconds((prev) => ({ ...prev, [m.user_id]: Number(e.target.value) }))
                              }
                            >
                              <option value={300}>5 分钟</option>
                              <option value={3600}>1 小时</option>
                              <option value={86400}>1 天</option>
                              <option value={604800}>7 天</option>
                              <option value={2592000}>30 天</option>
                              <option value={-1}>永久</option>
                            </select>
                            <button
                              onClick={() => {
                                const seconds = muteSeconds[m.user_id] ?? 3600;
                                void p.onMute(m.user_id, seconds === -1 ? null : seconds);
                              }}
                            >
                              禁言
                            </button>
                          </>
                        )}
                        {p.canManageMembers && (
                          <button className="danger" onClick={() => void p.onRemove(m.user_id)}>
                            移出
                          </button>
                        )}
                      </>
                    )}
                </div>
              )}
            </li>
            );
          })}
        </ul>
        {p.mutes.length > 0 && (
          <div>
            <h3>当前禁言</h3>
            <ul className="mute-list">
              {p.mutes.map((m) => {
                const target = m.scope === "room" ? null : p.members.find((x) => x.user_id === m.target_user_id);
                const who = m.scope === "room" ? "全员" : (target?.nickname || target?.username || m.target_user_id?.slice(0, 8) || "成员");
                return (
                  <li key={m.mute_id} className="mute-row">
                    <span className="mute-who" title={who}>{who}</span>
                    <span className="mute-until">
                      {m.expires_at ? `至 ${new Date(m.expires_at).toLocaleString()}` : "永久"}
                    </span>
                    <button className="link" onClick={() => void p.onUnmute(m.mute_id)}>解除</button>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </section>

      {p.canManageInvites && (
        <section className="card">
          <h2>邀请</h2>
          <div className="row-actions invite-actions">
            <select value={inviteExpiry} onChange={(e) => setInviteExpiry(e.target.value)}>
              <option value="1h">1 小时</option>
              <option value="6h">6 小时</option>
              <option value="24h">24 小时</option>
              <option value="7d">7 天</option>
              <option value="30d">30 天</option>
            </select>
            <button onClick={() => void createInvite()}>生成邀请链接</button>
          </div>
          {inviteUrl && (
            <p className="invite-url">
              <code>{location.origin + inviteUrl}</code>
              <button onClick={() => void copyInvite()}>
                {inviteCopied ? "已复制" : "复制"}
              </button>
              {inviteCopied && (
                <span className="copy-feedback" role="status">
                  邀请链接已复制
                </span>
              )}
            </p>
          )}
        </section>
      )}

      {p.isOwner &&
        p.webhooksEnabled &&
        (p.myRole === "admin" || p.myRole === "super_admin") &&
        p.room.kind === "standard" && (
          <Webhooks code={p.code} onError={p.onError} onNotice={p.onNotice} />
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
          <label>
            房间公告（空 = 不显示）
            <textarea
              value={settings.announcement}
              onChange={(e) => setSettings((s) => ({ ...s, announcement: e.target.value }))}
              maxLength={500}
              rows={2}
            />
          </label>
          <details className="advanced-settings">
            <summary>高级设置</summary>
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
          <label className="check">
            <input
              type="checkbox"
              checked={settings.e2ee}
              disabled={p.room.media_mode !== "sfu"}
              onChange={(e) => setSettings((x) => ({ ...x, e2ee: e.target.checked }))}
            />
            <span>
              媒体加密（服务器持钥，非端到端）
              <small>
                仅 SFU 房间可用；密钥由服务器生成并随 media-token 下发给成员。
                可防止 LiveKit 节点与中间网络窃听，但服务端仍可解密。
                切换后成员需重新加入语音生效。
              </small>
            </span>
          </label>
          {p.isOwner && (p.myRole === "admin" || p.myRole === "super_admin") && (
            <label className="check">
              <input
                type="checkbox"
                checked={settings.guest_upgrade_open}
                onChange={(e) =>
                  setSettings((x) => ({ ...x, guest_upgrade_open: e.target.checked }))
                }
              />
              <span>
                允许访客直接注册
                <small>
                  开启后，通过邀请链接加入本房间的访客可直接注册为正式用户；
                  失去管理员身份时自动关闭。
                </small>
              </span>
            </label>
          )}
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
                <option value={0}>自动</option>
                {availableTiers.map((t) => (
                  <option key={t.kbps} value={t.kbps}>
                    {t.kbps} kbps
                  </option>
                ))}
              </select>
            </label>
          )}
          {p.room.kind === "standard" && (
            <div className="settings-subsection">
              <h3>媒体模式</h3>
              <p className="hint">切换 P2P/SFU 需管理员审批；生效后请重新加入语音。</p>
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
                <option value={0}>自动</option>
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
            </div>
          )}
          <a className="settings-export" href={`${apiOrigin()}/api/v1/rooms/${p.code}/export`} download>
            <span className="settings-export-icon" aria-hidden="true">↓</span>
            <span>导出聊天记录</span>
            <small>JSON</small>
          </a>
          </details>
          {p.room.kind === "standard" && (
            <div className="settings-subsection">
<h3>全员禁言</h3>
          <p className="hint">开启后仅获准发言者可语音和发言。</p>
          <div className="row-actions">
            <button
              disabled={muteBusy}
              onClick={() => {
                toggleMute();
                setMuteBusy(true);
                window.setTimeout(() => setMuteBusy(false), 650);
              }}
            >
              {muteBusy
                ? "处理中…"
                : p.room.room_muted
                  ? "关闭全员禁言"
                  : "开启全员禁言"}
            </button>
          </div>
          {p.room.room_muted && (
            <>
              <h4>发言申请</h4>
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
            </div>
          )}
          <div className="settings-actions">
            <button onClick={() => void saveSettings()}>保存设置</button>
            <button className="danger" onClick={() => void deleteRoom()}>
              删除房间
            </button>
          </div>
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
        {
          method: "POST",
          body: JSON.stringify({
            username,
            password,
            ...(invite.trim() ? { invite_token: invite.trim() } : {}),
          }),
        },
      );
      // 等 Set-Cookie 落盘后再整页刷新：会话已切换为新账号，
      // 刷新后房间成员关系（已转移）让用户直接以正式身份回到房间。
      await new Promise<void>((resolve) => window.setTimeout(resolve, 300));
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
      <input
        placeholder="邀请码（可选，已获批准可留空）"
        value={invite}
        onChange={(e) => setInvite(e.target.value)}
      />
      <p className="hint">
        未填写邀请码时：需房主为你申请、管理员批准；若房主是管理员且已开启“允许访客直接注册”，可直接提交。
      </p>
      <button
        disabled={busy || !username || !password || !confirm}
        onClick={() => void doUpgrade()}
      >
        {busy ? "注册中…" : "注册并绑定"}
      </button>
    </div>
  );
}


