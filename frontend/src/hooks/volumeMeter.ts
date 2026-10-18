/**
 * 语音音量检测：每个 MediaStream 绑定独立的 AnalyserNode，周期性计算
 * 归一化音量（0..1）。用于"说话中"动效——自己/他人的头像绿圈随音量变化。
 * 语音激活灵敏度：低于阈值的音量归零（Discord 式 VAD 灵敏度调节）。
 */

const VAD_KEY = "cloudroom.vad";
let vadThreshold = loadVadThreshold();

function loadVadThreshold(): number {
  try {
    const v = Number(localStorage.getItem(VAD_KEY));
    if (Number.isFinite(v) && v > 0 && v <= 0.2) return v;
  } catch {
    /* 忽略 */
  }
  return 0.05;
}

/** 当前语音激活阈值（越小越灵敏）。 */
export function getVadThreshold(): number {
  return vadThreshold;
}

/** 设置语音激活阈值并持久化。 */
export function setVadThreshold(t: number): void {
  vadThreshold = Math.max(0.01, Math.min(0.2, t));
  try {
    localStorage.setItem(VAD_KEY, String(vadThreshold));
  } catch {
    /* 忽略 */
  }
}

export class VolumeMeter {
  private ctx: AudioContext;
  private meters = new Map<
    string,
    { src: MediaStreamAudioSourceNode; analyser: AnalyserNode; buf: Uint8Array<ArrayBuffer> }
  >();

  constructor() {
    this.ctx = new AudioContext();
  }

  /** 绑定一个流（同一 id 重复绑定会先解绑旧源）。 */
  attach(id: string, stream: MediaStream): void {
    if (this.meters.has(id)) this.detach(id);
    const src = this.ctx.createMediaStreamSource(stream);
    const analyser = this.ctx.createAnalyser();
    analyser.fftSize = 512;
    src.connect(analyser);
    this.meters.set(id, { src, analyser, buf: new Uint8Array(analyser.fftSize) });
  }

  detach(id: string): void {
    const m = this.meters.get(id);
    if (m) {
      m.src.disconnect();
      this.meters.delete(id);
    }
  }

  /** 单路音量 0..1（低于语音激活阈值归零）。 */
  level(id: string): number {
    const m = this.meters.get(id);
    if (!m) return 0;
    m.analyser.getByteTimeDomainData(m.buf);
    const v = rmsToLevel(m.buf);
    return v < vadThreshold ? 0 : v;
  }

  /** 全部已绑定流的音量。 */
  levels(): Record<string, number> {
    const out: Record<string, number> = {};
    for (const id of this.meters.keys()) {
      out[id] = this.level(id);
    }
    return out;
  }

  close(): void {
    for (const id of [...this.meters.keys()]) this.detach(id);
    void this.ctx.close();
  }
}

function rmsToLevel(buf: Uint8Array): number {
  let sum = 0;
  for (let i = 0; i < buf.length; i++) {
    const v = (buf[i] - 128) / 128;
    sum += v * v;
  }
  const rms = Math.sqrt(sum / buf.length);
  // 经验阈值：低于 0.02 视为静音，0.15 以上视为满格。
  return Math.min(1, Math.max(0, (rms - 0.02) / 0.15));
}
