import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import { VolumeMeter } from "./volumeMeter";
import { DenoiseEngine, getNoiseSettings, setNoisePrefs, subscribeNoise } from "./denoise";
import type { NoiseMode } from "./denoise";
import { openMicrophone as openMicCapture } from "./audioCapture";
import type { AecStatus } from "./audioCapture";
import { RemotePlayer, getDynamicsSettings, setVolumePref, subscribeDynamics } from "./remoteAudio";

/** 远端音频元素：元素负责解码（原生播放路径），RemotePlayer 只做增益/压缩后处理。 */
function attachAudioEl(els: Map<string, HTMLAudioElement>, id: string, track: MediaStreamTrack): HTMLAudioElement {
  let el = els.get(id);
  if (!el) {
    el = document.createElement("audio");
    el.autoplay = true;
    el.setAttribute("playsinline", "");
    document.body.appendChild(el);
    els.set(id, el);
  }
  el.srcObject = new MediaStream([track]);
  void el.play().catch(() => undefined);
  return el;
}
function detachAudioEl(els: Map<string, HTMLAudioElement>, id: string): void {
  const el = els.get(id);
  if (!el) return;
  el.pause();
  el.srcObject = null;
  el.remove();
  els.delete(id);
}

export type SignalData = {
  type: "offer" | "answer" | "ice";
  sdp?: string;
  candidate?: RTCIceCandidateInit;
};

/** 语音网络质量（取各 peer 最差）。 */
export type Quality = "good" | "medium" | "poor";

export interface P2PVoiceState {
  status: "idle" | "connecting" | "connected";
  inVoice: boolean;
  micMuted: boolean;
  speakerMuted: boolean;
  /** 用户侧降噪：off / webrtc（浏览器内置）/ rnnoise（WASM）。 */
  noiseMode: NoiseMode;
  /** 回声消除开关（默认开；双讲削波/外放媒体场景可关）。 */
  aec: boolean;
  /** 浏览器实际应用的回声消除状态。 */
  aecStatus: AecStatus;
  /** 自动增益开关（默认开；底噪放大/音乐动态压缩场景可关）。 */
  agc: boolean;
  join: () => Promise<void>;
  leave: () => void;
  toggleMicrophone: () => void;
  toggleSpeaker: () => void;
  /** 强制设置麦克风静音（PTT）。 */
  applyMicMuted: (muted: boolean) => void;
  setNoiseMode: (mode: NoiseMode) => void;
  setAec: (on: boolean) => void;
  setAgc: (on: boolean) => void;
  /** 调整某远端用户音量（0..2 = 0%..200%）。 */
  setRemoteVolume: (uid: string, v: number) => void;
  /** 网络质量（good/medium/poor，语音中采样）。 */
  quality: Quality | null;
  handleSignal: (from: string, data: SignalData) => void;
}

interface UseP2PVoiceArgs {
  code: string;
  enabled: boolean;
  connected: boolean;
  send: (payload: unknown) => boolean;
  voiceMembers: string[];
  myUserId: string | undefined;
  /** 音量检测回调：uid -> 归一化音量 0..1（120ms 节流）。 */
  onSpeakingChange?: (levels: Record<string, number>) => void;
}

/**
 * Browser-to-browser voice mesh for p2p rooms. SDP/ICE is relayed through the
 * CloudRoom control-plane WebSocket; ICE servers come exclusively from the
 * backend's `/ice-servers` endpoint, which mints LiveKit embedded-TURN
 * credentials (no independent coturn).
 */
export function useP2PVoice(args: UseP2PVoiceArgs): P2PVoiceState {
  const { code, enabled, connected, send, voiceMembers, myUserId, onSpeakingChange } = args;
  const [status, setStatus] = useState<"idle" | "connecting" | "connected">("idle");
  const [micMuted, setMicMuted] = useState(false);
  const [speakerMuted, setSpeakerMuted] = useState(false);
  const [noiseMode, setNoiseModeState] = useState<NoiseMode>(() => getNoiseSettings().mode);
  const [aec, setAecState] = useState<boolean>(() => getNoiseSettings().aec);
  const [aecStatus, setAecStatus] = useState<AecStatus>("unknown");
  const [agc, setAgcState] = useState<boolean>(() => getNoiseSettings().agc);
  const noiseModeRef = useRef(noiseMode);
  const aecRef = useRef(aec);
  const agcRef = useRef(agc);
  const denoiseRef = useRef<DenoiseEngine | null>(null);
  const speakerMutedRef = useRef(false);
  const meterRef = useRef<VolumeMeter | null>(null);
  const speakTimerRef = useRef<number | null>(null);
  const onSpeakingRef = useRef(onSpeakingChange);
  onSpeakingRef.current = onSpeakingChange;
  const peersRef = useRef<Map<string, RTCPeerConnection>>(new Map());
  const peerCandidatesRef = useRef<Map<string, RTCIceCandidateInit[]>>(new Map());
  const audioElsRef = useRef<Map<string, HTMLAudioElement>>(new Map());
  const playerRef = useRef<RemotePlayer | null>(null);
  const peerRecreateRef = useRef<Map<string, number>>(new Map());
  const iceServersRef = useRef<RTCIceServer[] | null>(null);
  const localStreamRef = useRef<MediaStream | null>(null);
  const rebuildSeqRef = useRef(0);
  const voiceOnRef = useRef(false);
  const myUserIdRef = useRef(myUserId);
  myUserIdRef.current = myUserId;

  const inVoice = voiceOnRef.current && status === "connected";

  const getIceServers = useCallback(async (): Promise<RTCIceServer[]> => {
    if (iceServersRef.current) return iceServersRef.current;
    try {
      const r = await api<{ iceServers: RTCIceServer[] }>(
        `/api/v1/rooms/${code}/ice-servers`,
        { method: "GET" },
      );
      iceServersRef.current = r.iceServers || [];
    } catch {
      iceServersRef.current = [];
    }
    return iceServersRef.current;
  }, [code]);

  const sendSignal = useCallback(
    (to: string, data: SignalData) => {
      send({ type: "signal", to, data });
    },
    [send],
  );

  const flushCandidates = useCallback((peerId: string, pc: RTCPeerConnection) => {
    const q = peerCandidatesRef.current.get(peerId) || [];
    peerCandidatesRef.current.set(peerId, []);
    for (const c of q) {
      void pc.addIceCandidate(c).catch(() => undefined);
    }
  }, []);

  const createPeer = useCallback(
    (peerId: string, initiator: boolean): RTCPeerConnection => {
      // iceCandidatePoolSize：预取候选，减少首包建立延迟（无副作用）。
      const pc = new RTCPeerConnection({
        iceServers: iceServersRef.current ?? [],
        iceCandidatePoolSize: 4,
      });
      peersRef.current.set(peerId, pc);
      if (localStreamRef.current) {
        localStreamRef.current.getTracks().forEach((t) => pc.addTrack(t, localStreamRef.current!));
      }
      // 只协商 Opus。此前优先 RED（RFC 2198）在部分手机/桌面组合上会出现
      // “track 已收到但音频为静音”的问题，因此不再启用 RED。
      try {
        const caps = RTCRtpSender.getCapabilities?.("audio");
        if (caps) {
          const opus = caps.codecs.find((c) => c.mimeType.toLowerCase() === "audio/opus");
          const audioTx = pc.getTransceivers().find((t) => t.sender.track?.kind === "audio");
          if (opus && audioTx && typeof audioTx.setCodecPreferences === "function") {
            audioTx.setCodecPreferences([opus]);
          }
        }
      } catch {
        /* 浏览器不支持 setCodecPreferences 时使用默认编码 */
      }
      pc.onicecandidate = (ev) => {
        if (ev.candidate) {
          sendSignal(peerId, { type: "ice", candidate: ev.candidate.toJSON() });
        }
      };
      pc.onconnectionstatechange = () => {
        // 对端刷新/消失：连接彻底失败时移除并重建 peer（5 秒退避防风暴）
        if (
          voiceOnRef.current &&
          (pc.connectionState === "failed" || pc.connectionState === "closed") &&
          !(pc as unknown as { _closing?: boolean })._closing
        ) {
          const last = peerRecreateRef.current.get(peerId) || 0;
          if (Date.now() - last < 5000) return;
          peerRecreateRef.current.set(peerId, Date.now());
          peersRef.current.delete(peerId);
          playerRef.current?.detach(peerId);
          detachAudioEl(audioElsRef.current, peerId);
          createPeer(peerId, initiator);
        }
      };
      pc.oniceconnectionstatechange = () => {
        if (pc.iceConnectionState === "failed" || pc.iceConnectionState === "disconnected") {
          // 候选丢失/网络抖动后重新协商（restartIce 必须配新的 offer 才真正生效）
          if ((pc as unknown as { _restarting?: boolean })._restarting) return;
          (pc as unknown as { _restarting?: boolean })._restarting = true;
          pc.createOffer({ iceRestart: true })
            .then(async (offer) => {
              await pc.setLocalDescription(offer);
              sendSignal(peerId, { type: "offer", sdp: offer.sdp || undefined });
            })
            .catch(() => undefined)
            .finally(() => {
              setTimeout(() => {
                (pc as unknown as { _restarting?: boolean })._restarting = false;
              }, 3000);
            });
        }
      };
      pc.ontrack = (ev) => {
        // WebAudio 播放链：每用户音量 + 可选音量均衡（压缩/限制）。
        // 直接用 ev.track 建单轨流，避免 ev.streams[0] 为空或含错轨导致静音。
        if (ev.track.kind !== "audio") return;
        const stream = new MediaStream([ev.track]);
        if (!stream.getAudioTracks().length) return;
        const el = attachAudioEl(audioElsRef.current, peerId, ev.track);
        const player = (playerRef.current ??= new RemotePlayer());
        player.resume();
        player.attach(peerId, el);
        const meter = (meterRef.current ??= new VolumeMeter());
        meter.resume();
        meter.attach(peerId, stream);
        // 对端刷新/关闭连接时远端流结束：立即重建 peer（5 秒退避）
        stream.getAudioTracks().forEach((t) => {
          t.onended = () => {
            if (!voiceOnRef.current) return;
            player.detach(peerId);
            const last = peerRecreateRef.current.get(peerId) || 0;
            if (Date.now() - last < 5000) return;
            peerRecreateRef.current.set(peerId, Date.now());
            const oldPc = peersRef.current.get(peerId);
            if (oldPc && oldPc !== pc) {
              (oldPc as unknown as { _closing?: boolean })._closing = true;
              oldPc.close();
            }
            peersRef.current.delete(peerId);
            createPeer(peerId, initiator);
          };
        });
      };
      if (initiator) {
        void pc.createOffer().then(async (offer) => {
          await pc.setLocalDescription(offer);
          sendSignal(peerId, { type: "offer", sdp: offer.sdp || undefined });
        });
      }
      return pc;
    },
    [sendSignal],
  );

  const handleSignal = useCallback(
    (from: string, data: SignalData) => {
      if (!voiceOnRef.current) return;
      let pc = peersRef.current.get(from);
      if (!pc) {
        pc = createPeer(from, false);
      }
      if (data.type === "offer" && data.sdp) {
        void pc
          .setRemoteDescription({ type: "offer", sdp: data.sdp })
          .then(async () => {
            flushCandidates(from, pc!);
            const answer = await pc!.createAnswer();
            await pc!.setLocalDescription(answer);
            sendSignal(from, { type: "answer", sdp: answer.sdp || undefined });
          })
          .catch(() => undefined);
      } else if (data.type === "answer" && data.sdp) {
        void pc
          .setRemoteDescription({ type: "answer", sdp: data.sdp })
          .then(() => flushCandidates(from, pc!))
          .catch(() => undefined);
      } else if (data.type === "ice" && data.candidate) {
        if (pc.remoteDescription) {
          void pc.addIceCandidate(data.candidate).catch(() => undefined);
        } else {
          // remoteDescription 未就绪时缓存，防止候选乱序丢失导致音质断续
          const q = peerCandidatesRef.current.get(from) || [];
          q.push(data.candidate);
          peerCandidatesRef.current.set(from, q);
        }
      }
    },
    [createPeer, flushCandidates, sendSignal],
  );

  // Voice presence: mesh — older members get offers from newer members.
  useEffect(() => {
    if (!voiceOnRef.current) return;
    const myIdx = voiceMembers.indexOf(myUserIdRef.current || "");
    for (const [i, uid] of voiceMembers.entries()) {
      if (uid === myUserIdRef.current) continue;
      if (!peersRef.current.has(uid)) {
        createPeer(uid, myIdx > i);
      }
    }
    // 清理已离开语音的 peer
    [...peersRef.current.keys()].forEach((uid) => {
      if (uid === myUserIdRef.current || voiceMembers.includes(uid)) return;
      const pc = peersRef.current.get(uid);
      if (pc) {
        (pc as unknown as { _closing?: boolean })._closing = true;
        pc.close();
      }
      peersRef.current.delete(uid);
      playerRef.current?.detach(uid);
      detachAudioEl(audioElsRef.current, uid);
    });
  }, [voiceMembers, createPeer]);

  const cleanup = useCallback(() => {
    voiceOnRef.current = false;
    setStatus("idle");
    setAecStatus("unknown");
    if (speakTimerRef.current !== null) {
      window.clearInterval(speakTimerRef.current);
      speakTimerRef.current = null;
    }
    meterRef.current?.close();
    meterRef.current = null;
    onSpeakingRef.current?.({});
    denoiseRef.current?.close();
    denoiseRef.current = null;
    if (localStreamRef.current) {
      localStreamRef.current.getTracks().forEach((t) => t.stop());
      localStreamRef.current = null;
    }
    peersRef.current.forEach((pc) => {
      (pc as unknown as { _closing?: boolean })._closing = true;
      pc.close();
    });
    peersRef.current.clear();
    peerCandidatesRef.current.clear();
    audioElsRef.current.forEach((el) => { el.pause(); el.srcObject = null; el.remove(); });
    audioElsRef.current.clear();
    playerRef.current?.close();
    playerRef.current = null;
  }, []);

  // 按当前降噪模式打开麦克风：rnnoise 模式先取原始流再过 WASM 管线。
  const openMicrophone = useCallback(async (): Promise<{
    stream: MediaStream;
    aecStatus: AecStatus;
  }> => {
    const mode = noiseModeRef.current;
    const { stream: raw, aecStatus } = await openMicCapture({
      aec: aecRef.current,
      mode,
      agc: agcRef.current,
    });
    if (mode !== "rnnoise") return { stream: raw, aecStatus };
    try {
      const engine = (denoiseRef.current ??= await DenoiseEngine.create());
      engine.attach(raw);
      // 保留原始流以便后续停止（engine 只引用不持有）。
      denoiseRef.current = engine;
      return { stream: engine.stream, aecStatus };
    } catch {
      // 设备不支持 48kHz：降级为浏览器降噪，不打断加入。
      denoiseRef.current = null;
      return { stream: raw, aecStatus };
    }
  }, []);

  const join = useCallback(async () => {
    if (!enabled || voiceOnRef.current) return;
    setStatus("connecting");
    // 在用户手势内创建 AudioContext，避免移动端自动播放策略把远端音频挂起。
    const playerCtx = (playerRef.current ??= new RemotePlayer());
    playerCtx.resume();
    const meterCtx = (meterRef.current ??= new VolumeMeter());
    meterCtx.resume();
    // P2P 直连失败时使用 LiveKit 内嵌 TURN 的短期凭证。
    await getIceServers();
    if (!connected) {
      setStatus("idle");
      throw new Error("连接尚未就绪，请重试");
    }
    try {
      const capture = await openMicrophone();
      localStreamRef.current = capture.stream;
      setAecStatus(capture.aecStatus);
    } catch {
      setStatus("idle");
      throw new Error("无法访问麦克风。");
    }
    if (myUserIdRef.current) {
      const meter = (meterRef.current ??= new VolumeMeter());
      meter.attach(myUserIdRef.current, localStreamRef.current);
    }
    if (speakTimerRef.current === null) {
      speakTimerRef.current = window.setInterval(() => {
        const meter = meterRef.current;
        if (!meter) return;
        const levels = meter.levels();
        onSpeakingRef.current?.(levels);
      }, 120);
    }
    voiceOnRef.current = true;
    send({ type: "voice_join" });
    setStatus("connected");
  }, [enabled, connected, getIceServers, send, openMicrophone]);

  const leave = useCallback(() => {
    if (voiceOnRef.current) {
      send({ type: "voice_leave" });
    }
    cleanup();
  }, [cleanup, send]);

  // 离开页面/禁用时清理；enabled 翻转（房间模式变更）或切换到其他房间
  // （SPA 内 /:code 组件复用）时安全退出旧会话，避免旧 peer/流残留。
  useEffect(() => {
    if (!enabled && voiceOnRef.current) {
      cleanup();
    }
    return () => {
      if (voiceOnRef.current) cleanup();
    };
  }, [enabled, code, cleanup]);

  const toggleMicrophone = useCallback(() => {
    const muted = !micMuted;
    setMicMuted(muted);
    localStreamRef.current?.getAudioTracks().forEach((t) => (t.enabled = !muted));
  }, [micMuted]);

  /** 强制设置麦克风静音（PTT 按住说话用；本地静音不重开设备）。 */
  const applyMicMuted = useCallback((muted: boolean) => {
    setMicMuted(muted);
    localStreamRef.current?.getAudioTracks().forEach((t) => (t.enabled = !muted));
  }, []);

  const toggleSpeaker = useCallback(() => {
    const muted = !speakerMutedRef.current;
    speakerMutedRef.current = muted;
    setSpeakerMuted(muted);
    playerRef.current?.setSpeakerMuted(muted);
  }, []);

  /** 调整某远端用户的音量（0..2）；未在语音中同样持久化偏好。 */
  const setRemoteVolume = useCallback((uid: string, v: number) => {
    setVolumePref(uid, v);
    playerRef.current?.setVolume(uid, v);
  }, []);

  // 网络质量采样（3s）：各 peer 音频丢包率与 RTT，取最差。
  const [quality, setQuality] = useState<Quality | null>(null);
  useEffect(() => {
    const iv = window.setInterval(() => {
      if (!voiceOnRef.current || peersRef.current.size === 0) return;
      void (async () => {
        let loss = 0;
        let rtt = 0;
        let peers = 0;
        for (const pc of peersRef.current.values()) {
          try {
            const stats = await pc.getStats();
            let pLoss = 0;
            let pRecv = 0;
            let pRtt = 0;
            stats.forEach((s) => {
              if (s.type === "inbound-rtp" && s.kind === "audio") {
                pLoss += s.packetsLost ?? 0;
                pRecv += s.packetsReceived ?? 0;
              }
              if (s.type === "transport") {
                pRtt = Math.max(pRtt, s.currentRoundTripTime ?? 0);
              }
            });
            loss = Math.max(loss, pRecv > 0 ? pLoss / pRecv : 0);
            rtt = Math.max(rtt, pRtt);
            peers++;
          } catch {
            /* 单 peer 采样失败忽略 */
          }
        }
        if (peers === 0) return;
        setQuality(loss < 0.01 && rtt < 0.15 ? "good" : loss < 0.05 && rtt < 0.4 ? "medium" : "poor");
      })();
    }, 3000);
    return () => {
      window.clearInterval(iv);
      setQuality(null);
    };
  }, []);

  // 音量均衡设置（个人设置模态框修改）：实时应用到播放链。
  useEffect(() => {
    return subscribeDynamics(() => {
      playerRef.current?.setDynamics(getDynamicsSettings());
    });
  }, []);

  // 语音中重建本地流并热替换到所有 peer（降噪/回声/增益切换共用）。
  const rebuildStream = useCallback(async () => {
    if (!voiceOnRef.current) return;
    const seq = ++rebuildSeqRef.current;
    // 先打开新流，成功后再停旧流；旧流在新流可用前保持发送，避免设置切换期间静音。
    let next: MediaStream;
    let nextAecStatus: AecStatus;
    try {
      const capture = await openMicrophone();
      next = capture.stream;
      nextAecStatus = capture.aecStatus;
    } catch {
      return;
    }
    if (seq !== rebuildSeqRef.current || !voiceOnRef.current) {
      next.getTracks().forEach((t) => t.stop());
      return;
    }
    const track = next.getAudioTracks()[0];
    if (!track) {
      next.getTracks().forEach((t) => t.stop());
      return;
    }
    try {
      await Promise.all(
        [...peersRef.current.values()].map(async (pc) => {
          const sender = pc.getSenders().find((s) => s.track?.kind === "audio");
          if (sender) await sender.replaceTrack(track);
          else pc.addTrack(track, next);
        }),
      );
    } catch {
      next.getTracks().forEach((t) => t.stop());
      return;
    }
    const old = localStreamRef.current;
    localStreamRef.current = next;
    setAecStatus(nextAecStatus);
    denoiseRef.current?.close();
    denoiseRef.current = null;
    old?.getTracks().forEach((t) => t.stop());
    // 音量检测重新绑定新流。
    if (myUserIdRef.current) {
      const meter = (meterRef.current ??= new VolumeMeter());
      meter.attach(myUserIdRef.current, next);
    }
  }, [openMicrophone]);

  // 切换降噪模式 / 回声消除 / 自动增益：持久化偏好；语音中重建流。
  const setNoiseMode = useCallback(
    (mode: NoiseMode) => {
      noiseModeRef.current = mode;
      setNoiseModeState(mode);
      setNoisePrefs({ mode });
      void rebuildStream();
    },
    [rebuildStream],
  );

  const setAec = useCallback(
    (on: boolean) => {
      aecRef.current = on;
      setAecState(on);
      setNoisePrefs({ aec: on });
      void rebuildStream();
    },
    [rebuildStream],
  );

  const setAgc = useCallback(
    (on: boolean) => {
      agcRef.current = on;
      setAgcState(on);
      setNoisePrefs({ agc: on });
      void rebuildStream();
    },
    [rebuildStream],
  );

  // 设置模态框等外部修改：订阅同步并重建（setter 自身已更新 ref，不会重复重建）。
  useEffect(() => {
    return subscribeNoise(() => {
      const s = getNoiseSettings();
      if (s.mode !== noiseModeRef.current) {
        noiseModeRef.current = s.mode;
        setNoiseModeState(s.mode);
        void rebuildStream();
      }
      if (s.aec !== aecRef.current) {
        aecRef.current = s.aec;
        setAecState(s.aec);
        void rebuildStream();
      }
      if (s.agc !== agcRef.current) {
        agcRef.current = s.agc;
        setAgcState(s.agc);
        void rebuildStream();
      }
    });
  }, [rebuildStream]);

  return {
    status,
    inVoice,
    micMuted,
    speakerMuted,
    noiseMode,
    aec,
    aecStatus,
    agc,
    join,
    leave,
    toggleMicrophone,
    toggleSpeaker,
    applyMicMuted,
    setNoiseMode,
    setAec,
    setAgc,
    setRemoteVolume,
    quality,
    handleSignal,
  };
}
