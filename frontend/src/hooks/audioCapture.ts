import type { NoiseMode } from "./denoise";

/** Actual state of browser echo cancellation for the captured track. */
export type AecStatus = "applied" | "unsupported" | "off" | "unknown";

export interface MicPreferences {
  aec: boolean;
  mode: NoiseMode;
  agc: boolean;
}

export interface OpenMicrophoneResult {
  stream: MediaStream;
  aecStatus: AecStatus;
}

/** Build the single capture constraint set used by both voice adapters. */
export function buildMicConstraints({ aec, mode, agc }: MicPreferences): MediaTrackConstraints {
  return {
    echoCancellation: aec,
    // rnnoise processes the raw stream itself; do not double-suppress noise.
    noiseSuppression: mode === "webrtc",
    autoGainControl: agc,
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
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: buildMicConstraints(prefs),
  });
  return {
    stream,
    aecStatus: await ensureEchoCancellation(stream, prefs.aec),
  };
}
