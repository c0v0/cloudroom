/**
 * Spike 001 浏览器侧探针。
 * 在 CloudRoom 前端 origin（默认 https://localhost:8081）内运行，
 * 因此 /api/v1 请求同源、带会话 cookie，无需额外 CORS 处理。
 *
 * 流程：登录/bootstrap 注册 -> 建房(p2p) -> GET /ice-servers ->
 *       两个 relay-only RTCPeerConnection 同页交换 SDP -> 断言选中 relay + 收包。
 */
window.__spike001 = async function (opts) {
  const { username, password, bootToken, roomCode } = opts;
  const log = [];

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

  // 1) 账号：优先登录；失败且日志里有 bootstrap 令牌则注册（首个用户 = super_admin）
  let auth = await post("/api/v1/auth/login", { username, password });
  if (auth.status !== 200 && bootToken) {
    auth = await post("/api/v1/auth/register", { username, password, token: bootToken });
  }
  log.push(["auth", auth.status]);
  if (auth.status !== 200) return { ok: false, stage: "auth", log, auth };

  const me = await api("/api/v1/me");
  const csrf = me.body && me.body.csrf_token;
  if (!csrf) return { ok: false, stage: "csrf", log, me };

  // 2) p2p 房间：超管免审批；房间已存在（409）直接复用
  const created = await post(
    "/api/v1/room-requests",
    { kind: "create_room", note: "spike-001", desired_code: roomCode, media_mode: "p2p" },
    csrf,
  );
  log.push(["room_request", created.status, created.body && created.body.code]);
  if (created.status !== 200 && created.status !== 409) {
    return { ok: false, stage: "room", log, created };
  }

  // 3) LiveKit 内嵌 TURN 短期凭证
  const ice = await api(`/api/v1/rooms/${roomCode}/ice-servers`);
  const iceServers = ice.body && ice.body.iceServers;
  log.push(["ice", ice.status, Array.isArray(iceServers) ? iceServers.length : 0]);
  if (!Array.isArray(iceServers) || iceServers.length === 0) {
    return { ok: false, stage: "ice", log, ice };
  }

  // 4) relay-only 双 PC（同页直接交换 SDP，隔离出 TURN 这一风险点）
  const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  const track = stream.getAudioTracks()[0];
  const mic = track.getSettings();
  const mk = () => new RTCPeerConnection({ iceServers, iceTransportPolicy: "relay" });
  const a = mk();
  const b = mk();
  a.addTrack(track, stream);
  b.addTrack(track, stream);

  const gatheringDone = (pc) =>
    new Promise((resolve) => {
      if (pc.iceGatheringState === "complete") return resolve();
      const t = setTimeout(resolve, 15000);
      pc.addEventListener("icegatheringstatechange", () => {
        if (pc.iceGatheringState === "complete") {
          clearTimeout(t);
          resolve();
        }
      });
    });

  const t0 = Date.now();
  const iceErrors = [];
  const onIceError = (e) => {
    iceErrors.push({ url: e.url, code: e.errorCode, text: e.errorText, host: e.hostCandidate });
  };
  a.addEventListener("icecandidateerror", onIceError);
  b.addEventListener("icecandidateerror", onIceError);
  await a.setLocalDescription(await a.createOffer());
  await gatheringDone(a);
  await b.setRemoteDescription(a.localDescription);
  await b.setLocalDescription(await b.createAnswer());
  await gatheringDone(b);
  await a.setRemoteDescription(b.localDescription);

  const connected = await new Promise((resolve) => {
    const t = setTimeout(() => resolve(false), 20000);
    const check = () => {
      if (["connected", "completed"].includes(a.iceConnectionState)) {
        clearTimeout(t);
        resolve(true);
      }
    };
    a.addEventListener("iceconnectionstatechange", check);
    check();
  });
  log.push(["connected", connected, Date.now() - t0 + "ms"]);

  await new Promise((r) => setTimeout(r, 2500)); // 等 RTP 计数

  const stats = await a.getStats();
  const cand = new Map();
  let pair = null;
  const localCandidates = [];
  stats.forEach((s) => {
    if (s.type === "local-candidate" || s.type === "remote-candidate") cand.set(s.id, s);
    if (s.type === "local-candidate") {
      localCandidates.push({ type: s.candidateType, protocol: s.protocol, address: s.address, port: s.port, url: s.url });
    }
    if (s.type === "candidate-pair" && s.state === "succeeded" && s.nominated) pair = s;
  });
  const sdpCandidates = (a.localDescription && a.localDescription.sdp ? a.localDescription.sdp : "")
    .split("\n")
    .filter((l) => l.startsWith("a=candidate"))
  const pairLocal = pair && cand.get(pair.localCandidateId) ? cand.get(pair.localCandidateId).candidateType : null;
  const pairRemote = pair && cand.get(pair.remoteCandidateId) ? cand.get(pair.remoteCandidateId).candidateType : null;

  const bstats = await b.getStats();
  let bytesReceived = 0;
  let packetsReceived = 0;
  let codec = null;
  bstats.forEach((s) => {
    if (s.type === "inbound-rtp" && s.kind === "audio") {
      bytesReceived = s.bytesReceived || 0;
      packetsReceived = s.packetsReceived || 0;
    }
    if (s.type === "codec" && (s.mimeType || "").includes("opus")) codec = s.mimeType;
  });

  const iceConnectionState = a.iceConnectionState;
  const iceGatheringState = a.iceGatheringState;
  a.close();
  b.close();
  stream.getTracks().forEach((t) => t.stop());

  return {
    ok: true,
    iceConnectionState,
    iceGatheringState,
    iceErrors,
    localCandidates,
    sdpCandidates,
    pairLocal,
    pairRemote,
    bytesReceived,
    packetsReceived,
    codec,
    micSampleRate: mic.sampleRate || null,
    iceServers: iceServers.map((s) => (Array.isArray(s.urls) ? s.urls.join(",") : s.urls)),
    log,
  };
};
