import {
  createDenoiseEngine,
  isWasmNoiseMode,
  noiseErrorMessage,
  setNoiseRuntimeStatus,
  type NoiseEngine,
  type NoiseMode,
} from "./denoise";
import { openMicrophone } from "./audioCapture";
import type { AecStatus, MicPreferences, OpenMicrophoneResult } from "./audioCapture";

export interface ProcessedMic {
  stream: MediaStream;
  aecStatus: AecStatus;
  actualMode: NoiseMode;
  engine: NoiseEngine | null;
}

/** AEC 约束可能导致部分设备/浏览器直接拒绝采集；失败时退回关闭 AEC，保住输入。 */
async function captureWithAecFallback(prefs: MicPreferences): Promise<OpenMicrophoneResult> {
  try {
    return await openMicrophone(prefs);
  } catch (error) {
    if (!prefs.aec) throw error;
    const message = error instanceof Error ? error.message : String(error);
    if (!/Overconstrained|NotSupported|Constraint|echo/i.test(message)) throw error;
    const fallback = await openMicrophone({ ...prefs, aec: false });
    return { ...fallback, aecStatus: "unsupported" };
  }
}

/** 打开麦克风并按所选模型处理；失败时显式回退到 WebRTC 原生降噪。 */
export async function openProcessedMicrophone(
  prefs: MicPreferences,
  previous: NoiseEngine | null,
): Promise<ProcessedMic> {
  const requested = prefs.mode;
  if (!isWasmNoiseMode(requested)) {
    previous?.close();
    setNoiseRuntimeStatus({
      requested,
      actual: requested,
      state: "ready",
      message: requested === "webrtc" ? "已启用浏览器自带降噪" : "降噪已关闭",
    });
    const capture = await captureWithAecFallback(prefs);
    return {
      stream: capture.stream,
      aecStatus: capture.aecStatus,
      actualMode: requested,
      engine: null,
    };
  }

  setNoiseRuntimeStatus({
    requested,
    actual: requested,
    state: "loading",
    message: "正在启动降噪…",
  });

  let engine: NoiseEngine | null = previous && previous.mode === requested ? previous : null;
  if (previous && previous.mode !== requested) {
    previous.close();
  }

  let capture: OpenMicrophoneResult | null = null;
  try {
    capture = await captureWithAecFallback(prefs);
    engine ??= await createDenoiseEngine(requested);
    engine.attach(capture.stream);
    setNoiseRuntimeStatus({
      requested,
      actual: requested,
      state: "ready",
      message: "降噪已开启",
    });
    return {
      stream: engine.stream,
      aecStatus: capture.aecStatus,
      actualMode: requested,
      engine,
    };
  } catch (error) {
    capture?.stream.getTracks().forEach((t) => t.stop());
    engine?.close();
    const detail = noiseErrorMessage(error);
    try {
      const fallback = await captureWithAecFallback({ ...prefs, mode: "webrtc" });
      setNoiseRuntimeStatus({
        requested,
        actual: "webrtc",
        state: "fallback",
        message: `${detail}；已改用 WebRTC 降噪`,
      });
      return {
        stream: fallback.stream,
        aecStatus: fallback.aecStatus,
        actualMode: "webrtc",
        engine: null,
      };
    } catch (fallbackError) {
      setNoiseRuntimeStatus({
        requested,
        actual: "off",
        state: "error",
        message: `降噪启动失败：${detail}`,
      });
      throw fallbackError;
    }
  }
}
