/** 消息提示音开关：模块级缓存 + 订阅，避免每次渲染读 localStorage。 */
const KEY = "cloudroom.sound";
let enabled = read();
const listeners = new Set<(v: boolean) => void>();

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

export function soundEnabled(): boolean {
  return enabled;
}

export function setSoundEnabled(v: boolean): void {
  enabled = v;
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    /* 忽略 */
  }
  for (const fn of listeners) fn(v);
}

export function subscribeSound(fn: (v: boolean) => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
