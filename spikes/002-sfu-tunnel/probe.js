/**
 * Spike 002 浏览器侧探针（单页面内两个 LiveKit 会话）。
 * 页面在 CloudRoom 前端 origin 内（同源 cookie），livekit-client 用 UMD 注入
 * （window.LivekitClient），信令走 frontend-server 的 /rtc/ WSS 隧道。
 *
 * 顺序：alice 登录/注册 -> 建房 -> 发邀请 -> 取 alice media-token
 *       -> bob 注册/入房 -> 取 bob media-token -> 两个 Room 同时连接/发布。
 */
window.__spike002 = (() => {
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const state = { rooms: {}, tracks: {} };

  const api = async (path, init) => {
    const r = await fetch(path, { credentials: "include", ...(init || {}) });
    const text = await r.text();
    let body = null;
    try {
      body = JSON.parse(text);
    } catch {
      body = text;
    }
    return { status: r.status, body };
  };
  const post = (path, body, csrf) =>
    api(path, {
      method: "POST",
      headers: {
        "content-type": "application/json",
        ...(csrf ? { "x-csrf-token": csrf } : {}),
      },
      body: JSON.stringify(body || {}),
    });

  const ensureAccount = async (username, password, token) => {
    let auth = await post("/api/v1/auth/login", { username, password });
    if (auth.status !== 200 && token) {
      auth = await post("/api/v1/auth/register", { username, password, token });
    }
    if (auth.status !== 200) return { ok: false, stage: "auth", auth };
    const me = await api("/api/v1/me");
    const csrf = me.body && me.body.csrf_token;
    if (!csrf) return { ok: false, stage: "csrf", me };
    return { ok: true, csrf };
  };

  const localAudio = (name) => {
    const room = state.rooms[name];
    const pub = [...(room.localParticipant.audioTrackPublications.values() || [])][0];
    return pub && pub.track;
  };

  const remoteAudio = (name) => {
    const room = state.rooms[name];
    for (const p of room.remoteParticipants.values()) {
      for (const pub of p.audioTrackPublications.values()) {
        if (pub.track) return { identity: p.identity, track: pub.track };
      }
    }
    return null;
  };

  return {
    state,

    /** alice：登录/注册（超管）-> 建 sfu 房 -> 房间邀请 + 注册邀请。 */
    async aliceSetup({ username, password, bootToken, roomCode }) {
      const acc = await ensureAccount(username, password, bootToken);
      if (!acc.ok) return acc;
      const created = await post(
        "/api/v1/room-requests",
        { kind: "create_room", note: "spike-002", desired_code: roomCode, media_mode: "sfu" },
        acc.csrf,
      );
      if (created.status !== 200 && created.status !== 409) {
        return { ok: false, stage: "room", created };
      }
      const roomInvite = await post(`/api/v1/rooms/${roomCode}/invites`, { max_role: "member" }, acc.csrf);
      const regInvite = await post("/api/v1/admin/invites", {}, acc.csrf);
      return {
        ok: true,
        csrf: acc.csrf,
        roomInvite: roomInvite.body && roomInvite.body.invite && roomInvite.body.invite.token,
        regInvite: regInvite.body && regInvite.body.token,
        roomStatus: created.status,
      };
    },

    /** bob：注册/登录 -> 凭房间邀请加入。 */
    async bobSetup({ username, password, regToken, inviteToken, roomCode }) {
      const acc = await ensureAccount(username, password, regToken);
      if (!acc.ok) return acc;
      const join = await post(`/api/v1/rooms/${roomCode}/join`, { invite_token: inviteToken }, acc.csrf);
      if (join.status !== 200 && join.status !== 409) return { ok: false, stage: "join", join };
      return { ok: true, csrf: acc.csrf, joinStatus: join.status };
    },

    async mediaToken({ csrf, roomCode }) {
      const r = await post(`/api/v1/rooms/${roomCode}/media-token`, {}, csrf);
      if (r.status !== 200) return r;
      return { status: r.status, body: r.body };
    },

    /** 连接 LiveKit（/rtc/ 隧道）并按 48kHz/指定码率发布麦克风。 */
    async connect({ name, url, token, bitrate, sampleRate, websocketTimeout, peerConnectionTimeout }) {
      const { Room } = window.LivekitClient;
      const room = new Room({
        websocketTimeout: websocketTimeout || 15000,
        peerConnectionTimeout: peerConnectionTimeout || 15000,
      });
      state.rooms[name] = room;
      await room.connect(url, token);
      await room.localParticipant.setMicrophoneEnabled(true, { sampleRate }, { bitrate, dtx: false });
      const track = localAudio(name);
      state.tracks[name] = track;
      return {
        ok: true,
        name,
        state: room.state,
        identity: room.localParticipant.identity,
        url,
        mic: track ? track.mediaStreamTrack.getSettings() : null,
      };
    },

    /** 等远端音轨出现并开始收包。 */
    async waitRemoteAudio({ name, timeoutMs }) {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const found = remoteAudio(name);
        if (found) {
          const s1 = await found.track.getReceiverStats();
          await sleep(1500);
          const s2 = await found.track.getReceiverStats();
          if (s2 && (s2.bytesReceived || 0) > (s1 ? s1.bytesReceived || 0 : 0)) {
            return {
              ok: true,
              name,
              identity: found.identity,
              bytesReceived: s2.bytesReceived || 0,
              settings: found.track.mediaStreamTrack.getSettings(),
            };
          }
        }
        await sleep(300);
      }
      return { ok: false, name, stage: "remote-audio-timeout" };
    },

    /** 发送端码率（bytesSent 差分）与音频轨道设置。 */
    async measure({ name, sampleMs }) {
      const track = state.tracks[name];
      if (!track) return { ok: false, name, stage: "no-local-track" };
      const s1 = await track.getSenderStats();
      await sleep(sampleMs);
      const s2 = await track.getSenderStats();
      if (!s1 || !s2) return { ok: false, name, stage: "no-sender-stats" };
      const kbps = ((s2.bytesSent - s1.bytesSent) * 8) / (sampleMs / 1000) / 1000;
      return {
        ok: true,
        name,
        kbps: Math.round(kbps * 10) / 10,
        bytesSent: s2.bytesSent,
        packetsSent: s2.packetsSent,
        mic: track.mediaStreamTrack.getSettings(),
        connectionState: state.rooms[name].state,
      };
    },

    async disconnect() {
      for (const room of Object.values(state.rooms)) {
        try {
          await room.disconnect();
        } catch {
          /* 忽略断开异常 */
        }
      }
    },
  };
})();
