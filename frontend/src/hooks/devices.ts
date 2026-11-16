/** 麦克风/扬声器选择：用户默认级（localStorage）+ 当前标签页级（sessionStorage）。 */
const MIC_KEY = "cloudroom.micId";
const SPK_KEY = "cloudroom.spkId";
const listeners = new Set<Listener>();
export type DeviceScope = "default" | "tab";
type Listener = () => void;

function readFrom(storage: Storage, key: string): string | null {
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}
function writeTo(storage: Storage, key: string, value: string | null): void {
  try {
    if (value === null) storage.removeItem(key);
    else storage.setItem(key, value);
  } catch {
    /* 隐私模式/存储被禁用时忽略 */
  }
}
function readLocal(key: string): string {
  return readFrom(localStorage, key) ?? "";
}
/** 标签页级覆盖：null 表示没有覆盖，空字符串表示“本标签页用系统默认”。 */
function readTab(key: string): string | null {
  return readFrom(sessionStorage, key);
}
function notify(): void {
  for (const fn of listeners) fn();
}
function resolve(key: string): string {
  const tab = readTab(key);
  return tab !== null ? tab : readLocal(key);
}
function setPreference(key: string, id: string, scope: DeviceScope): void {
  if (scope === "tab") {
    writeTo(sessionStorage, key, id);
  } else {
    writeTo(localStorage, key, id || null);
    writeTo(sessionStorage, key, null);
  }
  notify();
}

export function getMicId(): string {
  return resolve(MIC_KEY);
}
export function setMicId(id: string, scope: DeviceScope = getDeviceScope()): void {
  setPreference(MIC_KEY, id, scope);
}
export function getSpeakerId(): string {
  return resolve(SPK_KEY);
}
export function setSpeakerId(id: string, scope: DeviceScope = getDeviceScope()): void {
  setPreference(SPK_KEY, id, scope);
}
/** 当前标签页是否启用了临时覆盖。 */
export function getDeviceScope(): DeviceScope {
  return readTab(MIC_KEY) !== null || readTab(SPK_KEY) !== null ? "tab" : "default";
}
/** 切换作用范围，并把当前实际选择复制到新范围。 */
export function setDeviceScope(scope: DeviceScope): void {
  const mic = getMicId();
  const spk = getSpeakerId();
  if (scope === "tab") {
    writeTo(sessionStorage, MIC_KEY, mic);
    writeTo(sessionStorage, SPK_KEY, spk);
  } else {
    writeTo(localStorage, MIC_KEY, mic || null);
    writeTo(localStorage, SPK_KEY, spk || null);
    writeTo(sessionStorage, MIC_KEY, null);
    writeTo(sessionStorage, SPK_KEY, null);
  }
  notify();
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
/** 输出设备是否可切换（当前播放链走 AudioContext）。 */
export function canSelectSpeaker(): boolean {
  return typeof AudioContext !== "undefined" && "setSinkId" in AudioContext.prototype;
}
/** 给单个 <audio> 元素应用扬声器选择；浏览器不支持时静默忽略。 */
export function applySinkId(el: HTMLMediaElement, deviceId = getSpeakerId()): void {
  const target = el as HTMLMediaElement & { setSinkId?: (id: string) => Promise<void> };
  if (typeof target.setSinkId !== "function") return;
  void target.setSinkId(deviceId || "").catch(() => undefined);
}
