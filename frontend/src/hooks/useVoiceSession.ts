import { useCallback, useMemo, useState } from "react";
import type { RoomSocket } from "./useRoomSocket";
import { useP2PVoice } from "./useP2PVoice";
import type { Quality, SignalData } from "./useP2PVoice";
import { useLiveKitVoice } from "./useLiveKitVoice";
import type { LiveKitVoiceStatus } from "./useLiveKitVoice";
import type { AecStatus } from "./audioCapture";
import type { NoiseMode } from "./denoise";

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
  /** 说话动效：uid（=用户 ID）-> 归一化音量 0..1。 */
  speaking: Record<string, number>;
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
  /** 网络质量（good/medium/poor）。 */
  quality: Quality | null;
  handleSignal: (from: string, data: SignalData) => void;
  audioContainerRef: React.RefObject<HTMLDivElement>;
}

interface UseVoiceSessionArgs {
  code: string;
  mode: "p2p" | "sfu" | null;
  myUserId: string | undefined;
  socket: RoomSocket;
  /** 房间音频码率（kbps，0=自动），SFU 生效。 */
  audioBitrate?: number;
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
  const { code, mode, myUserId, socket, audioBitrate, onSpeakingChange } = args;
  const [speaking, setSpeaking] = useState<Record<string, number>>({});

  // 音量回调：更新本 hook 的 speaking 状态，并透传给外部调用方。
  const handleSpeaking = useCallback(
    (levels: Record<string, number>) => {
      setSpeaking(levels);
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
      speaking,
      join: mode === "sfu" ? lk.join : p2p.join,
      leave: mode === "sfu" ? lk.leave : p2p.leave,
      toggleMicrophone: mode === "sfu" ? lk.toggleMicrophone : p2p.toggleMicrophone,
      toggleSpeaker: mode === "sfu" ? lk.toggleSpeaker : p2p.toggleSpeaker,
      applyMicMuted: mode === "sfu" ? lk.applyMicMuted : p2p.applyMicMuted,
      setNoiseMode: mode === "sfu" ? lk.setNoiseMode : p2p.setNoiseMode,
      setAec: mode === "sfu" ? lk.setAec : p2p.setAec,
      setAgc: mode === "sfu" ? lk.setAgc : p2p.setAgc,
      setRemoteVolume: mode === "sfu" ? lk.setRemoteVolume : p2p.setRemoteVolume,
      quality: mode === "sfu" ? lk.quality : p2p.quality,
      handleSignal: p2p.handleSignal,
      audioContainerRef: lk.audioContainerRef,
    }),
    [mode, p2p, lk, socket.voiceMembers, speaking],
  );
}

export type { LiveKitVoiceStatus };
