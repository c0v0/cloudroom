import { useCallback, useMemo } from "react";
import type { RoomSocket } from "./useRoomSocket";
import { useP2PVoice } from "./useP2PVoice";
import type { Quality, SignalData } from "./useP2PVoice";
import { useLiveKitVoice } from "./useLiveKitVoice";
import type { LiveKitVoiceStatus } from "./useLiveKitVoice";
import type { AecStatus } from "./audioCapture";
import type { NoiseMode } from "./denoise";

import { setSpeakingLevels } from "../lib/speakingStore";

export interface VoiceSessionState {
  mode: "p2p" | "sfu" | null;
  status: "idle" | "connecting" | "connected" | "reconnecting";
  inVoice: boolean;
  participantCount: number;
  micMuted: boolean;
  speakerMuted: boolean;
  /** 用户侧降噪：off / webrtc / rnnoise。 */
  noiseMode: NoiseMode;
  /** 回声消除开关。 */
  aec: boolean;
  /** 当前适配器实际应用的 AEC 状态。 */
  aecStatus: AecStatus;
  /** 自动增益开关。 */
  agc: boolean;
  join: () => Promise<void>;
  leave: () => void;
  toggleMicrophone: () => void;
  toggleSpeaker: () => void;
  /** 强制设置麦克风静音（PTT）。 */
  applyMicMuted: (muted: boolean) => void;
  /** PTT 按下时绕过发送门限。 */
  setPttActive: (active: boolean) => void;
  setNoiseMode: (mode: NoiseMode) => void;
  setAec: (on: boolean) => void;
  setAgc: (on: boolean) => void;
  /** 调整某远端用户音量（0..2 = 0%..200%）。 */
  setRemoteVolume: (uid: string, v: number) => void;
  /** 网络质量（good/medium/poor）。 */
  quality: Quality | null;
  handleSignal: (from: string, data: SignalData) => void;
  audioContainerRef: React.RefObject<HTMLDivElement>;
  /** 屏幕共享（仅 SFU 房间可用）。 */
  screenSharing: boolean;
  toggleScreenShare: () => Promise<void>;
  screenContainerRef: React.RefObject<HTMLDivElement>;
  /** 本地录音（两种媒体模式都可用）。 */
  startRecording: () => MediaStream | null;
  stopRecording: () => void;
}

interface UseVoiceSessionArgs {
  code: string;
  mode: "p2p" | "sfu" | null;
  myUserId: string | undefined;
  socket: RoomSocket;
  /** 房间音频码率（kbps，0=自动），SFU 生效。 */
  audioBitrate?: number;
  /** 房间是否开启媒体加密。 */
  e2ee?: boolean;
  /** 音量检测回调（说话动效）。 */
  onSpeakingChange?: (levels: Record<string, number>) => void;
}

/**
 * Unified voice session: activates exactly one adapter according to the
 * room's current media_mode. p2p rooms use the WebSocket-relayed mesh with
 * LiveKit embedded-TURN credentials; sfu rooms use livekit-client directly.
 * When the approved mode changes, the old adapter is torn down and the UI
 * asks the user to rejoin under the new mode (never a silent switch).
 */
export function useVoiceSession(args: UseVoiceSessionArgs): VoiceSessionState {
  const { code, mode, myUserId, socket, audioBitrate, e2ee, onSpeakingChange } = args;

  // 音量回调：写入模块级 store（避免整个房间随电平重渲染），并透传外部调用方。
  const handleSpeaking = useCallback(
    (levels: Record<string, number>) => {
      setSpeakingLevels(levels);
      onSpeakingChange?.(levels);
    },
    [onSpeakingChange],
  );

  const p2p = useP2PVoice({
    code,
    enabled: mode === "p2p",
    connected: socket.connected,
    send: socket.send,
    voiceMembers: socket.voiceMembers,
    myUserId,
    onSpeakingChange: handleSpeaking,
  });
  const lk = useLiveKitVoice({
    code,
    enabled: mode === "sfu",
    audioBitrate,
    e2ee,
    onSpeakingChange: handleSpeaking,
  });

  return useMemo<VoiceSessionState>(
    () => ({
      mode,
      status: mode === "sfu" ? lk.status : p2p.status,
      inVoice: mode === "sfu" ? lk.inVoice : p2p.inVoice,
      participantCount: mode === "sfu" ? lk.participantIds.length : socket.voiceMembers.length,
      micMuted: mode === "sfu" ? lk.micMuted : p2p.micMuted,
      speakerMuted: mode === "sfu" ? lk.speakerMuted : p2p.speakerMuted,
      noiseMode: mode === "sfu" ? lk.noiseMode : p2p.noiseMode,
      aec: mode === "sfu" ? lk.aec : p2p.aec,
      aecStatus: mode === "sfu" ? lk.aecStatus : p2p.aecStatus,
      agc: mode === "sfu" ? lk.agc : p2p.agc,
      join: mode === "sfu" ? lk.join : p2p.join,
      leave: mode === "sfu" ? lk.leave : p2p.leave,
      toggleMicrophone: mode === "sfu" ? lk.toggleMicrophone : p2p.toggleMicrophone,
      toggleSpeaker: mode === "sfu" ? lk.toggleSpeaker : p2p.toggleSpeaker,
      applyMicMuted: mode === "sfu" ? lk.applyMicMuted : p2p.applyMicMuted,
      setPttActive: mode === "sfu" ? lk.setPttActive : p2p.setPttActive,
      setNoiseMode: mode === "sfu" ? lk.setNoiseMode : p2p.setNoiseMode,
      setAec: mode === "sfu" ? lk.setAec : p2p.setAec,
      setAgc: mode === "sfu" ? lk.setAgc : p2p.setAgc,
      setRemoteVolume: mode === "sfu" ? lk.setRemoteVolume : p2p.setRemoteVolume,
      quality: mode === "sfu" ? lk.quality : p2p.quality,
      handleSignal: p2p.handleSignal,
      audioContainerRef: lk.audioContainerRef,
      screenSharing: lk.screenSharing,
      toggleScreenShare: lk.toggleScreenShare,
      screenContainerRef: lk.screenContainerRef,
      startRecording: mode === "sfu" ? lk.startRecording : p2p.startRecording,
      stopRecording: mode === "sfu" ? lk.stopRecording : p2p.stopRecording,
    }),
    // 依赖逐字段列出：p2p/lk 每次渲染都是新对象，直接依赖它们会让 memo 永远失效。
    // 列全所有被引用的值，既不丢失更新，也能在状态未变时保持引用稳定。
    [
      mode,
      lk.status,
      lk.inVoice,
      lk.participantIds,
      lk.micMuted,
      lk.speakerMuted,
      lk.noiseMode,
      lk.aec,
      lk.aecStatus,
      lk.agc,
      lk.join,
      lk.leave,
      lk.toggleMicrophone,
      lk.toggleSpeaker,
      lk.applyMicMuted,
      lk.setPttActive,
      lk.setNoiseMode,
      lk.setAec,
      lk.setAgc,
      lk.setRemoteVolume,
      lk.quality,
      lk.audioContainerRef,
      lk.screenSharing,
      lk.toggleScreenShare,
      lk.screenContainerRef,
      lk.startRecording,
      lk.stopRecording,
      p2p.status,
      p2p.inVoice,
      p2p.micMuted,
      p2p.speakerMuted,
      p2p.noiseMode,
      p2p.aec,
      p2p.aecStatus,
      p2p.agc,
      p2p.join,
      p2p.leave,
      p2p.toggleMicrophone,
      p2p.toggleSpeaker,
      p2p.applyMicMuted,
      p2p.setPttActive,
      p2p.setNoiseMode,
      p2p.setAec,
      p2p.setAgc,
      p2p.setRemoteVolume,
      p2p.quality,
      p2p.handleSignal,
      p2p.startRecording,
      p2p.stopRecording,
      socket.voiceMembers,
    ],
  );
}

export type { LiveKitVoiceStatus };
