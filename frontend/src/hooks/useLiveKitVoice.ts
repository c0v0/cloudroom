import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import type { Room } from "livekit-client";
import { api } from "../api";
import { VolumeMeter, levelsChanged } from "./volumeMeter";
import { DenoiseEngine, getNoiseSettings, setNoisePrefs, subscribeNoise } from "./denoise";
import type { NoiseMode } from "./denoise";
import { ensureEchoCancellation, openMicrophone as openMicCapture } from "./audioCapture";
import type { AecStatus } from "./audioCapture";
import { RemotePlayer, getDynamicsSettings, setVolumePref, subscribeDynamics } from "./remoteAudio";
import { getMicId, getSpeakerId, subscribeDevices } from "./devices";
import type { Quality } from "./useP2PVoice";

export type LiveKitVoiceStatus = "idle" | "connecting" | "connected" | "reconnecting";

/**
 * 把 livekit-client 的底层错误翻译成用户可理解的中文提示。
 * "could not establish signal connection: Failed to fetch" 常见于
 * WebSocket 建连失败后客户端用跨域 fetch 探测服务器被浏览器拦截，
 * 底层原因通常是 LiveKit 地址不可达或 ws/wss 与页面协议不匹配。
 */
export function voiceErrorMessage(raw: string): string {
  if (
    /could not establish signal connection|Failed to fetch|server was not reachable|network/i.test(
      raw,
    )
  ) {
    return "无法连接语音服务，请确认 LiveKit 地址可访问（或联系管理员）。";
  }
  if (/Requested device not found|NotFoundError|NotAllowedError|PermissionDenied|permission/i.test(raw)) {
    return "无法使用麦克风：未找到设备或权限被拒绝，请检查浏览器麦克风权限。";
  }
  if (/403|not allowed|token/i.test(raw)) {
    return "语音连接被拒绝（令牌无效或房间权限不足）。";
  }
  if (/404|does not exist/i.test(raw)) {
    return "语音房间不存在，请刷新后重试。";
  }
  return raw;
}

export interface LiveKitVoiceState {
  status: LiveKitVoiceStatus;
  inVoice: boolean;
  participantIds: string[];
  micMuted: boolean;
  speakerMuted: boolean;
  /** 用户侧降噪：off / webrtc（浏览器内置）/ rnnoise（WASM）。 */
  noiseMode: NoiseMode;
  /** 回声消除开关（默认开）。 */
  aec: boolean;
  /** 浏览器实际应用的回声消除状态。 */
  aecStatus: AecStatus;
  /** 自动增益开关（默认开）。 */
  agc: boolean;
  join: () => Promise<void>;
  leave: () => void;
  toggleMicrophone: () => void;
  toggleSpeaker: () => void;
  /** 屏幕共享（仅 SFU；浏览器会弹选择器）。 */
  screenSharing: boolean;
  toggleScreenShare: () => Promise<void>;
  screenContainerRef: React.RefObject<HTMLDivElement>;
  /** 本地录音：混入远端 + 本地麦克风，返回可交给 MediaRecorder 的流。 */
  startRecording: () => MediaStream | null;
  stopRecording: () => void;
  /** 强制设置麦克风静音（PTT）。 */
  applyMicMuted: (muted: boolean) => void;
  setNoiseMode: (mode: NoiseMode) => void;
  setAec: (on: boolean) => void;
  setAgc: (on: boolean) => void;
  /** 调整某远端参与者音量（0..2 = 0%..200%）。 */
  setRemoteVolume: (uid: string, v: number) => void;
  /** 网络质量（good/medium/poor，连接中有效）。 */
  quality: Quality | null;
  audioContainerRef: RefObject<HTMLDivElement>;
}

interface UseLiveKitVoiceArgs {
  code: string;
  enabled: boolean;
  /** 房间音频码率（kbps，0=自动）。 */
  audioBitrate?: number;
  /** 房间开启了媒体加密（仅 SFU）。 */
  e2ee?: boolean;
  /** 音量检测回调：participant identity（=用户 ID）-> 归一化音量 0..1。 */
  onSpeakingChange?: (levels: Record<string, number>) => void;
}

/**
 * LiveKit voice session for sfu rooms: requests a short-lived JWT from
 * CloudRoom's `/media-token`, connects with `livekit-client`, and publishes
 * only the microphone. Track/participant lifecycle is owned here; the control
 * plane WebSocket is not involved in media signaling.
 */
export function useLiveKitVoice(args: UseLiveKitVoiceArgs): LiveKitVoiceState {
  const { code, enabled, audioBitrate = 0, e2ee = false, onSpeakingChange } = args;
  // 去掉"自动"档后默认 64 kbps；旧数据 0（auto）按 64 发布。
  const effBitrate = audioBitrate || 48;
  const [status, setStatus] = useState<LiveKitVoiceStatus>("idle");
  const [participantIds, setParticipantIds] = useState<string[]>([]);
  const [micMuted, setMicMuted] = useState(false);
  const [speakerMuted, setSpeakerMuted] = useState(false);
  const [quality, setQuality] = useState<Quality | null>(null);
  const [screenSharing, setScreenSharing] = useState(false);
  const screenContainerRef = useRef<HTMLDivElement | null>(null) as React.RefObject<HTMLDivElement>;
  const screenElRef = useRef<HTMLVideoElement | null>(null);
  const [noiseMode, setNoiseModeState] = useState<NoiseMode>(() => getNoiseSettings().mode);
  const [aec, setAecState] = useState<boolean>(() => getNoiseSettings().aec);
  const [aecStatus, setAecStatus] = useState<AecStatus>("unknown");
  const [agc, setAgcState] = useState<boolean>(() => getNoiseSettings().agc);
  const noiseModeRef = useRef(noiseMode);
  const aecRef = useRef(aec);
  const agcRef = useRef(agc);
  const denoiseRef = useRef<DenoiseEngine | null>(null);
  /** 串行化麦克风重建，避免快速切换模式时旧重建把新轨重新打开。 */
  const rebuildSeqRef = useRef(0);
  const speakerMutedRef = useRef(false);
  const micMutedRef = useRef(false);
  const roomRef = useRef<Room | null>(null);
  const playerRef = useRef<RemotePlayer | null>(null);
  const meterRef = useRef<VolumeMeter | null>(null);
  const speakTimerRef = useRef<number | null>(null);
  const prevLevelsRef = useRef<Record<string, number>>({});
  const onSpeakingRef = useRef(onSpeakingChange);
  onSpeakingRef.current = onSpeakingChange;
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;
  const audioContainerRef = useRef<HTMLDivElement>(null);

  const inVoice = !!roomRef.current && (status === "connected" || status === "reconnecting");

  const detachAll = useCallback(() => {
    playerRef.current?.close();
    playerRef.current = null;
  }, []);

  /** 统一的会话清理：join 失败、服务端断开、主动离开都走这里。 */
  const teardownSession = useCallback(() => {
    if (speakTimerRef.current !== null) {
      window.clearInterval(speakTimerRef.current);
      speakTimerRef.current = null;
    }
    meterRef.current?.close();
    meterRef.current = null;
    onSpeakingRef.current?.({});
    prevLevelsRef.current = {};
    denoiseRef.current?.close();
    denoiseRef.current = null;
    screenElRef.current?.remove();
    screenElRef.current = null;
    setScreenSharing(false);
    detachAll();
    setParticipantIds([]);
    setStatus("idle");
    setAecStatus("unknown");
  }, [detachAll]);

  const join = useCallback(async () => {
    if (!enabledRef.current || roomRef.current) return;
    // 在用户手势内创建 AudioContext，避免移动端自动播放策略把远端音频挂起。
    // livekit-client 按需加载：P2P 房间不会下载这段 ~530 kB 的 chunk。
    const { ExternalE2EEKeyProvider, Room, RoomEvent, Track } = await import(
      "livekit-client"
    );
    const playerCtx = (playerRef.current ??= new RemotePlayer());
    playerCtx.resume();
    // 加入前已切换扬声器静音时，新播放器也要继承该状态。
    playerCtx.setSpeakerMuted(speakerMutedRef.current);
    setStatus("connecting");
    const meterCtx = (meterRef.current ??= new VolumeMeter());
    meterCtx.resume();
    // E2EE：密钥由服务端在 media-token 中下发给成员（LiveKit 与中间网络无法解密）。
    const keyProvider = e2ee ? new ExternalE2EEKeyProvider() : null;
    const e2eeWorker = e2ee
      ? new (await import("livekit-client/e2ee-worker?worker")).default()
      : null;
    const lk = new Room({
      audioCaptureDefaults: {
        // 回声消除/自动增益由用户设置控制（有副作用场景可关）。
        echoCancellation: aecRef.current,
        // rnnoise 模式由 WASM 管线降噪，浏览器降噪关闭避免双重处理。
        noiseSuppression: noiseModeRef.current === "webrtc",
        autoGainControl: agcRef.current,
      },
      ...(keyProvider && e2eeWorker
        ? { e2ee: { keyProvider, worker: e2eeWorker } }
        : {}),
    });
    roomRef.current = lk;
    // 移动端/自动播放策略：用户任意一次点击时解锁远端音频播放。
    const unlockAudio = () => {
      void lk.startAudio().catch(() => undefined);
    };
    document.addEventListener("pointerdown", unlockAudio, { once: true });

    const refreshParticipants = () => {
      const ids = [...lk.remoteParticipants.keys()];
      if (lk.localParticipant) ids.push(lk.localParticipant.identity);
      setParticipantIds(ids);
    };

    lk.on(RoomEvent.TrackSubscribed, (track, _pub, participant) => {
      if (track.kind === "video") {
        // 屏幕共享：只渲染画面，音频仍走麦克风轨。
        if (_pub?.source !== Track.Source.ScreenShare) return;
        const el = track.attach() as HTMLVideoElement;
        el.autoplay = true;
        el.playsInline = true;
        el.muted = true;
        el.className = "screen-share-video";
        const box = screenContainerRef.current;
        if (box) {
          box.innerHTML = "";
          box.appendChild(el);
          box.classList.add("active");
        }
        screenElRef.current = el;
        return;
      }
      if (track.kind !== "audio" || !participant) return;
      // 直接用实际音频轨建流，避免 mediaStream 为空或含错轨导致静音。
      const stream = new MediaStream([track.mediaStreamTrack]);
      if (!stream.getAudioTracks().length) return;
      // 元素负责解码（原生 <audio>），WebAudio 只做增益/压缩/电平。
      const el = track.attach();
      const player = (playerRef.current ??= new RemotePlayer());
      player.resume();
      player.setSpeakerMuted(speakerMutedRef.current);
      player.attach(participant.identity, el);
      // 音量检测：参与者的说话动效
      const meter = (meterRef.current ??= new VolumeMeter());
      meter.resume();
      meter.attach(participant.identity, stream);
    });
    lk.on(RoomEvent.TrackUnsubscribed, (_track, _pub, participant) => {
      if (_track.kind === "video") {
        _track.detach();
        const box = screenContainerRef.current;
        if (box) {
          box.innerHTML = "";
          box.classList.remove("active");
        }
        screenElRef.current = null;
        return;
      }
      if (!participant) return;
      playerRef.current?.detach(participant.identity);
      _track.detach();
      meterRef.current?.detach(participant.identity);
    });
    // 网络质量：LiveKit 服务器计算，取本地+远端最差。
    const qualitiesRef = new Map<string, string>();
    const updateQuality = () => {
      let worst: string | null = null;
      for (const q of qualitiesRef.values()) {
        if (q === "lost" || (q === "poor" && worst !== "lost")) worst = q;
        else if (q === "poor") worst = q;
        else if (q === "medium" && worst !== "lost" && worst !== "poor") worst = q;
      }
      setQuality(
        worst === "lost" || worst === "poor"
          ? "poor"
          : worst === "medium"
            ? "medium"
            : worst === "excellent" || worst === "good"
              ? "good"
              : null,
      );
    };
    lk.on(RoomEvent.ConnectionQualityChanged, (q, participant) => {
      qualitiesRef.set(participant.identity, q);
      updateQuality();
    });
    lk.on(RoomEvent.ParticipantConnected, refreshParticipants);
    lk.on(RoomEvent.ParticipantDisconnected, refreshParticipants);
    // 浏览器自带的"停止共享"条不会经过按钮：用轨道事件保持状态一致。
    lk.on(RoomEvent.LocalTrackPublished, (pub) => {
      if (pub.source === Track.Source.ScreenShare) setScreenSharing(true);
    });
    lk.on(RoomEvent.LocalTrackUnpublished, (pub) => {
      if (pub.source === Track.Source.ScreenShare) {
        setScreenSharing(false);
        screenElRef.current?.remove();
        screenElRef.current = null;
      }
    });
    lk.on(RoomEvent.Reconnecting, () => setStatus("reconnecting"));
    lk.on(RoomEvent.Reconnected, () => {
      refreshParticipants();
      setStatus("connected");
    });
    lk.on(RoomEvent.Disconnected, () => {
      if (roomRef.current !== lk) return;
      roomRef.current = null;
      teardownSession();
    });

    let livekitUrl = "";
    try {
      const r = await api<{
        token: string;
        url: string;
        room: string;
        e2ee_key?: string | null;
      }>(`/api/v1/rooms/${code}/media-token`, { method: "POST", body: "{}" });
      livekitUrl = r.url;
      if (keyProvider && r.e2ee_key) {
        await keyProvider.setKey(r.e2ee_key);
      }
      await lk.connect(r.url, r.token);
      // 只发布麦克风；camera/screen-share/data 均不启用。
      // rnnoise 模式：先取原始流过 WASM 管线，再把处理后轨道交给 LiveKit 发布。
      const mode = noiseModeRef.current;
      let processedTrack: MediaStreamTrack | undefined;
      let captureAecStatus: AecStatus | undefined;
      if (mode === "rnnoise") {
        try {
          const capture = await openMicCapture({
            aec: aecRef.current,
            mode: "rnnoise",
            agc: agcRef.current,
            deviceId: getMicId() || undefined,
          });
          captureAecStatus = capture.aecStatus;
          const engine = (denoiseRef.current ??= await DenoiseEngine.create());
          engine.attach(capture.stream);
          denoiseRef.current = engine;
          processedTrack = engine.stream.getAudioTracks()[0];
        } catch (e) {
          // 设备不支持 48kHz：回退到浏览器降噪发布。
          console.warn("[voice] rnnoise 初始化失败，已回退浏览器降噪", e);
        }
      }
      await lk.localParticipant.setMicrophoneEnabled(
        true,
        {
          echoCancellation: aecRef.current,
          noiseSuppression: mode === "webrtc" || (mode === "rnnoise" && !processedTrack),
          autoGainControl: agcRef.current,
          ...(getMicId() ? { deviceId: getMicId() } : {}),
          ...(processedTrack ? { track: processedTrack } : {}),
        },
        {
          dtx: getDynamicsSettings().dtxOff ? false : undefined,
          ...(effBitrate > 0 ? { bitrate: effBitrate * 1000 } : {}),
        },
      );
      const localStream = [...lk.localParticipant.audioTrackPublications.values()]
        .find((pub) => pub.track?.kind === "audio")
        ?.track?.mediaStream;
      // 离开语音再重连时沿用静音状态：否则按钮显示"取消静音"但麦克风在发送。
      if (micMutedRef.current) {
        lk.localParticipant.audioTrackPublications.forEach((pub) => {
          pub.track?.mediaStream?.getAudioTracks().forEach((t) => (t.enabled = false));
        });
      }
      if (localStream) {
        setAecStatus(
          processedTrack && captureAecStatus
            ? captureAecStatus
            : await ensureEchoCancellation(localStream, aecRef.current),
        );
        const meter = (meterRef.current ??= new VolumeMeter());
        meter.attach(lk.localParticipant.identity, localStream);
        if (micMutedRef.current) meter.detach(lk.localParticipant.identity);
      }
      if (speakTimerRef.current === null) {
        speakTimerRef.current = window.setInterval(() => {
          const meter = meterRef.current;
          if (!meter) return;
          const levels = meter.levels();
          if (levelsChanged(prevLevelsRef.current, levels)) {
            prevLevelsRef.current = levels;
            onSpeakingRef.current?.(levels);
          }
        }, 150);
      }
      refreshParticipants();
      // iOS Safari 必须在用户手势内调用 startAudio 才能播放远端音频。
      try {
        await lk.startAudio();
      } catch {
        /* 不支持时忽略 */
      }
      setStatus("connected");
    } catch (e) {
      const stale = roomRef.current;
      roomRef.current = null;
      teardownSession();
      // join 失败时 Room 对象已经连上（或正在连），必须显式断开，
      // 否则会留下幽灵参与者与泄漏的 AudioContext/worker。
      if (stale) void stale.disconnect();
      const raw = e instanceof Error ? e.message : String(e);
      if (
        /could not establish signal connection|Failed to fetch|server was not reachable|network|ERR_CONNECTION/i.test(
          raw,
        )
      ) {
        throw new Error(
          `无法连接语音服务（${livekitUrl} 不可达）。请确认浏览器能访问该地址，或联系管理员检查 LiveKit 配置。`,
          { cause: e },
        );
      }
      throw new Error(voiceErrorMessage(raw), { cause: e });
    }
  }, [code, effBitrate, e2ee, teardownSession]);

  const leave = useCallback(() => {
    const lk = roomRef.current;
    roomRef.current = null;
    teardownSession();
    if (lk) {
      void lk.disconnect();
    }
  }, [teardownSession]);

  // 房间模式变更、切换到其他房间（SPA 内 /:code 组件复用）或页面卸载时
  // 清理 LiveKit 会话，防止旧房间连接/轨道残留导致新房间 join 失败。
  useEffect(() => {
    if (!enabled && roomRef.current) {
      leave();
    }
    return () => {
      if (roomRef.current) leave();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, code]);

  /** 强制设置麦克风静音（PTT 按住说话用）：本地静音，不重开设备/发布。 */
  const applyMicMuted = useCallback((muted: boolean) => {
    micMutedRef.current = muted;
    setMicMuted(muted);
    const lp = roomRef.current?.localParticipant;
    if (!lp) return;
    for (const pub of lp.audioTrackPublications.values()) {
      pub.track?.mediaStream?.getAudioTracks().forEach((t) => (t.enabled = !muted));
    }
    const meter = meterRef.current;
    if (meter) {
      if (muted) {
        meter.detach(lp.identity);
      } else {
        const localStream = [...lp.audioTrackPublications.values()]
          .find((pub) => pub.track?.kind === "audio")
          ?.track?.mediaStream;
        if (localStream) meter.attach(lp.identity, localStream);
      }
    }
  }, []);

  const toggleMicrophone = useCallback(() => {
    if (!roomRef.current?.localParticipant) return;
    // 只切 track.enabled：避免重新采集导致 rnnoise/码率/DTX 设置丢失。
    applyMicMuted(!micMutedRef.current);
  }, [applyMicMuted]);

  const toggleSpeaker = useCallback(() => {
    const muted = !speakerMutedRef.current;
    speakerMutedRef.current = muted;
    setSpeakerMuted(muted);
    playerRef.current?.setSpeakerMuted(muted);
  }, []);

  /** 调整某远端参与者的音量（0..2）；未在语音中同样持久化偏好。 */
  const setRemoteVolume = useCallback((uid: string, v: number) => {
    setVolumePref(uid, v);
    playerRef.current?.setVolume(uid, v);
  }, []);

  // 语音中重开本地麦克风（保留当前静音状态），按最新设置重建（rnnoise 用处理后轨道）。
  const rebuildMic = useCallback(async () => {
    const lk = roomRef.current;
    const lp = lk?.localParticipant;
    if (!lk || !lp) return;
    const seq = ++rebuildSeqRef.current;
    const mode = noiseModeRef.current;
    // 先停旧发布，再按新模式重建。
    await lp.setMicrophoneEnabled(false).catch(() => undefined);
    if (seq !== rebuildSeqRef.current) return;
    // 让浏览器先释放上一轮采集的音频处理状态。
    await new Promise((resolve) => setTimeout(resolve, 50));
    let processedTrack: MediaStreamTrack | undefined;
    let captureAecStatus: AecStatus | undefined;
    if (mode === "rnnoise") {
      try {
        const capture = await openMicCapture({
          aec: aecRef.current,
          mode: "rnnoise",
          agc: agcRef.current,
          deviceId: getMicId() || undefined,
        });
        captureAecStatus = capture.aecStatus;
        const engine = (denoiseRef.current ??= await DenoiseEngine.create());
        engine.attach(capture.stream);
        denoiseRef.current = engine;
        processedTrack = engine.stream.getAudioTracks()[0];
      } catch (e) {
        // 降级：浏览器降噪。
        console.warn("[voice] rnnoise 重建失败，已回退浏览器降噪", e);
      }
    } else {
      denoiseRef.current?.close();
      denoiseRef.current = null;
    }
    if (seq !== rebuildSeqRef.current) return;
    await lp
      .setMicrophoneEnabled(
        true,
        {
          echoCancellation: aecRef.current,
          noiseSuppression: mode === "webrtc" || (mode === "rnnoise" && !processedTrack),
          autoGainControl: agcRef.current,
          ...(getMicId() ? { deviceId: getMicId() } : {}),
          ...(processedTrack ? { track: processedTrack } : {}),
        },
        {
          dtx: getDynamicsSettings().dtxOff ? false : undefined,
          ...(effBitrate > 0 ? { bitrate: effBitrate * 1000 } : {}),
        },
      )
      .catch(() => undefined);
    const localStream = [...lp.audioTrackPublications.values()]
      .find((pub) => pub.track?.kind === "audio")
      ?.track?.mediaStream;
    if (localStream) {
      setAecStatus(
        processedTrack && captureAecStatus
          ? captureAecStatus
          : await ensureEchoCancellation(localStream, aecRef.current),
      );
    }
  }, [effBitrate]);

  // 音量均衡设置（个人设置模态框修改）：实时应用到播放链；
  // 关闭静音压缩（DTX）切换需重开麦克风发布才生效。
  const prevDtxOffRef = useRef(getDynamicsSettings().dtxOff);
  useEffect(() => {
    return subscribeDynamics(() => {
      const dyn = getDynamicsSettings();
      if (dyn.dtxOff !== prevDtxOffRef.current) {
        prevDtxOffRef.current = dyn.dtxOff;
        void rebuildMic();
      }
    });
  }, [rebuildMic]);

  // 设备选择：扬声器立即切换；麦克风变化时在语音中重建发布。
  const micIdRef = useRef(getMicId());
  useEffect(() => {
    return subscribeDevices(() => {
      playerRef.current?.setOutputDevice(getSpeakerId());
      const next = getMicId();
      if (next !== micIdRef.current) {
        micIdRef.current = next;
        void rebuildMic();
      }
    });
  }, [rebuildMic]);

  const setNoiseMode = useCallback(
    (mode: NoiseMode) => {
      noiseModeRef.current = mode;
      setNoiseModeState(mode);
      setNoisePrefs({ mode });
      void rebuildMic();
    },
    [rebuildMic],
  );

  const setAec = useCallback(
    (on: boolean) => {
      aecRef.current = on;
      setAecState(on);
      setNoisePrefs({ aec: on });
      void rebuildMic();
    },
    [rebuildMic],
  );

  const setAgc = useCallback(
    (on: boolean) => {
      agcRef.current = on;
      setAgcState(on);
      setNoisePrefs({ agc: on });
      void rebuildMic();
    },
    [rebuildMic],
  );

  // 设置模态框等外部修改：订阅同步并重建（setter 自身已更新 ref，不会重复重建）。
  useEffect(() => {
    return subscribeNoise(() => {
      const s = getNoiseSettings();
      if (s.mode !== noiseModeRef.current) {
        noiseModeRef.current = s.mode;
        setNoiseModeState(s.mode);
        void rebuildMic();
      }
      if (s.aec !== aecRef.current) {
        aecRef.current = s.aec;
        setAecState(s.aec);
        void rebuildMic();
      }
      if (s.agc !== agcRef.current) {
        agcRef.current = s.agc;
        setAgcState(s.agc);
        void rebuildMic();
      }
    });
  }, [rebuildMic]);

  const toggleScreenShare = useCallback(async () => {
    const lk = roomRef.current;
    if (!lk) return;
    const next = !screenSharing;
    try {
      await lk.localParticipant.setScreenShareEnabled(next, { audio: false });
      setScreenSharing(next);
    } catch (e) {
      setScreenSharing(false);
      throw new Error(
        next
          ? "无法开始屏幕共享：浏览器未授权或当前环境不支持。"
          : "无法停止屏幕共享。",
        { cause: e },
      );
    }
  }, [screenSharing]);

  const startRecording = useCallback(() => {
    const player = (playerRef.current ??= new RemotePlayer());
    const lk = roomRef.current;
    const tracks = lk
      ? [...lk.localParticipant.audioTrackPublications.values()]
          .map((p) => p.track?.mediaStreamTrack)
          .filter((t): t is MediaStreamTrack => !!t)
      : [];
    const local = tracks.length > 0 ? new MediaStream(tracks) : null;
    return player.startRecording(local);
  }, []);

  const stopRecording = useCallback(() => {
    playerRef.current?.stopRecording();
  }, []);

  return {
    status,
    inVoice,
    participantIds,
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
    screenSharing,
    toggleScreenShare,
    screenContainerRef,
    startRecording,
    stopRecording,
    applyMicMuted,
    setNoiseMode,
    setAec,
    setAgc,
    setRemoteVolume,
    quality,
    audioContainerRef,
  };
}
