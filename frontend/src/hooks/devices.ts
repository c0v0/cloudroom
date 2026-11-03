/** 麦克风/扬声器选择：localStorage 持久化 + 订阅，供语音会话即时应用。 */

const MIC_KEY = "cloudroom.micId";
const SPK_KEY = "cloudroom.spkId";

type Listener = () => void;
const listeners = new Set<Listener>();

function read(key: string): string {
  try {
    return localStorage.getItem(key) || "";
  } catch {
    return "";
  }
}

function write(key: string, value: string): void {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch {
    /* 隐私模式下忽略 */
  }
  for (const fn of listeners) fn();
}

export function getMicId(): string {
  return read(MIC_KEY);
}

export function setMicId(id: string): void {
  write(MIC_KEY, id);
}

export function getSpeakerId(): string {
  return read(SPK_KEY);
}

export function setSpeakerId(id: string): void {
  write(SPK_KEY, id);
}

export function subscribeDevices(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export interface AudioDevices {
  mics: MediaDeviceInfo[];
  speakers: MediaDeviceInfo[];
}

/** 枚举音频设备；未授权时 label 为空，由 UI 用序号兜底。 */
export async function listAudioDevices(): Promise<AudioDevices> {
  if (!navigator.mediaDevices?.enumerateDevices) return { mics: [], speakers: [] };
  try {
    const all = await navigator.mediaDevices.enumerateDevices();
    return {
      mics: all.filter((d) => d.kind === "audioinput"),
      speakers: all.filter((d) => d.kind === "audiooutput"),
    };
  } catch {
    return { mics: [], speakers: [] };
  }
}

/** 输出设备是否可切换（Chrome 110+ 支持 AudioContext.setSinkId）。 */
export function canSelectSpeaker(): boolean {
  return typeof AudioContext !== "undefined" && "setSinkId" in AudioContext.prototype;
}

/** 给单个 <audio> 元素应用扬声器选择；浏览器不支持时静默忽略。 */
export function applySinkId(el: HTMLMediaElement, deviceId = getSpeakerId()): void {
  const target = el as HTMLMediaElement & { setSinkId?: (id: string) => Promise<void> };
  if (typeof target.setSinkId !== "function") return;
  void target.setSinkId(deviceId || "").catch(() => undefined);
}
