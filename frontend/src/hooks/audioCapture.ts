import type { NoiseMode } from "./denoise";

/** Actual state of browser echo cancellation for the captured track. */
export type AecStatus = "applied" | "unsupported" | "off" | "unknown";

export interface MicPreferences {
  aec: boolean;
  mode: NoiseMode;
  agc: boolean;
  /** 指定输入设备；为空时使用系统默认。 */
  deviceId?: string;
}

export interface OpenMicrophoneResult {
  stream: MediaStream;
  aecStatus: AecStatus;
}

/** Build the single capture constraint set used by both voice adapters. */
export function buildMicConstraints({ aec, mode, agc, deviceId }: MicPreferences): MediaTrackConstraints {
  return {
    echoCancellation: aec,
    // WASM providers process the raw stream themselves; do not double-suppress noise.
    noiseSuppression: mode === "webrtc",
    autoGainControl: agc,
    ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
  };
}

/**
 * Browser AEC is a best-effort constraint. Confirm that the selected track
 * actually has it, and make one application attempt if the initial request
 * was ignored. This never throws into the join flow: callers only warn.
 */
export async function ensureEchoCancellation(
  stream: MediaStream,
  requested: boolean,
  extra?: MediaTrackConstraints,
): Promise<AecStatus> {
  if (!requested) return "off";
  const track = stream.getAudioTracks()[0];
  if (!track) return "unknown";

  try {
    if (track.getSettings().echoCancellation === true) return "applied";
  } catch {
    return "unsupported";
  }

  try {
    // 一次带上其他处理约束，避免浏览器只应用 echoCancellation 时把
    // autoGainControl/noiseSuppression 重置成默认值。
    await track.applyConstraints({ echoCancellation: true, ...extra });
  } catch {
    return "unsupported";
  }
  try {
    return track.getSettings().echoCancellation === true ? "applied" : "unsupported";
  } catch {
    return "unsupported";
  }
}

/**
 * 浏览器可能忽略初始的 autoGainControl 约束；在 AEC 调整之后再确认一次，
 * 避免“切到 WebRTC 后音量变小，重新开关 AGC 才恢复”。
 */
export async function ensureAutoGainControl(
  stream: MediaStream,
  requested: boolean,
  extra?: MediaTrackConstraints,
): Promise<void> {
  const track = stream.getAudioTracks()[0];
  if (!track) return;
  try {
    const actual = track.getSettings().autoGainControl === true;
    if (actual === requested) return;
    // 带上 AEC/降噪约束，避免某些浏览器只应用 AGC 时把其他开关重置。
    await track.applyConstraints({ autoGainControl: requested, ...extra });
  } catch {
    /* 尽力而为，不阻塞采集 */
  }
}

/** Open the microphone with the requested processing preferences. */
export async function openMicrophone(prefs: MicPreferences): Promise<OpenMicrophoneResult> {
  // 指定了设备就必须使用该设备；不能静默回退到系统默认，
  // 否则用户选了 CABLE，浏览器却可能打开 Realtek 麦克风阵列。
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: buildMicConstraints(prefs),
  });
  const track = stream.getAudioTracks()[0];
  const actualDeviceId = track?.getSettings().deviceId;
  if (prefs.deviceId && actualDeviceId && actualDeviceId !== prefs.deviceId) {
    stream.getTracks().forEach((t) => t.stop());
    throw new Error("所选麦克风没有生效，请重新选择输入设备。");
  }
  const aecStatus = await ensureEchoCancellation(stream, prefs.aec, {
    autoGainControl: prefs.agc,
    noiseSuppression: prefs.mode === "webrtc",
  });
  // AEC 的 applyConstraints 可能重置其他处理开关，最后再确认 AGC。
  await ensureAutoGainControl(stream, prefs.agc, {
    echoCancellation: prefs.aec,
    noiseSuppression: prefs.mode === "webrtc",
  });
  return {
    stream,
    aecStatus,
  };
}
