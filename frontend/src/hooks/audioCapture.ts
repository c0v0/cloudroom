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
    await track.applyConstraints({ echoCancellation: true });
  } catch {
    return "unsupported";
  }
  try {
    return track.getSettings().echoCancellation === true ? "applied" : "unsupported";
  } catch {
    return "unsupported";
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
  return {
    stream,
    aecStatus: await ensureEchoCancellation(stream, prefs.aec),
  };
}
