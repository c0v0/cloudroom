/**
 * 语音音量检测：每个 MediaStream 绑定独立的 AnalyserNode，周期性计算
 * 归一化音量（0..1）。用于"说话中"动效——自己/他人的头像绿圈随音量变化。
 * 语音激活灵敏度：低于阈值的音量归零（Discord 式 VAD 灵敏度调节）。
 */

const VAD_KEY = "cloudroom.vad";
/** 说话动效阈值的可调范围（dBFS）：-48 最灵敏，-24 最迟钝。 */
export const VAD_MIN_DB = -48;
export const VAD_MAX_DB = -24;
const VAD_LEVEL_MIN = 0.01;
const VAD_LEVEL_MAX = 0.8;
let vadThreshold = loadVadThreshold();

/** 归一化阈值 → dBFS（与 rmsToLevel 的 -50..-12 dB 映射一致）。 */
export function thresholdToDb(threshold: number): number {
  return Math.max(VAD_MIN_DB, Math.min(VAD_MAX_DB, Math.round(threshold * 38 - 50)));
}

/** dBFS → 归一化阈值，限制在可调范围内。 */
export function dbToThreshold(db: number): number {
  return Math.max(VAD_LEVEL_MIN, Math.min(VAD_LEVEL_MAX, (db + 50) / 38));
}

function loadVadThreshold(): number {
  try {
    const v = Number(localStorage.getItem(VAD_KEY));
    if (Number.isFinite(v) && v >= VAD_LEVEL_MIN && v <= VAD_LEVEL_MAX) return v;
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
  vadThreshold = Math.max(VAD_LEVEL_MIN, Math.min(VAD_LEVEL_MAX, t));
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
    {
      src: MediaStreamAudioSourceNode;
      analyser: AnalyserNode;
      buf: Uint8Array<ArrayBuffer>;
      sink: GainNode;
      smoothed: number;
    }
  >();

  constructor() {
    this.ctx = new AudioContext();
    this.resume();
  }

  /** 移动端/自动播放策略：在用户手势或绑定流时恢复 AudioContext。 */
  resume(): void {
    if (this.ctx.state === "suspended") void this.ctx.resume();
  }

  /** 绑定一个流（同一 id 重复绑定会先解绑旧源）。 */
  attach(id: string, stream: MediaStream): void {
    this.resume();
    if (this.meters.has(id)) this.detach(id);
    const src = this.ctx.createMediaStreamSource(stream);
    const analyser = this.ctx.createAnalyser();
    analyser.fftSize = 512;
    src.connect(analyser);
    // 零增益输出：让 analyser 成为活跃音频图的一部分，远端 track 才会被消费/解除 muted。
    const sink = this.ctx.createGain();
    sink.gain.value = 0;
    analyser.connect(sink);
    sink.connect(this.ctx.destination);
    this.meters.set(id, { src, analyser, buf: new Uint8Array(analyser.fftSize), sink, smoothed: 0 });
  }

  detach(id: string): void {
    const m = this.meters.get(id);
    if (m) {
      m.src.disconnect();
      m.sink.disconnect();
      this.meters.delete(id);
    }
  }

  /** 单路音量 0..1（低于语音激活阈值归零）。 */
  level(id: string): number {
    const m = this.meters.get(id);
    if (!m) return 0;
    m.analyser.getByteTimeDomainData(m.buf);
    const target = rmsToLevel(m.buf);
    // 快起慢落：起音立即跟上，收音缓慢回落，避免动效抖动。
    const k = target > m.smoothed ? 0.6 : 0.25;
    m.smoothed += (target - m.smoothed) * k;
    return m.smoothed < vadThreshold ? 0 : m.smoothed;
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
  if (rms < 1e-4) return 0;
  // 按 dBFS 映射（贴近人耳响度感知）：-50 dB 视为静音，-12 dB 视为满格。
  // 线性 RMS 会把正常说话压到 0.1~0.3，动效几乎不动。
  const db = 20 * Math.log10(rms);
  return Math.min(1, Math.max(0, (db + 50) / 38));
}

/** 电平表变化超过阈值才算变化：避免每帧 setState 触发整页重渲染。 */
export function levelsChanged(
  prev: Record<string, number>,
  next: Record<string, number>,
  epsilon = 0.02,
): boolean {
  const prevKeys = Object.keys(prev);
  const nextKeys = Object.keys(next);
  if (prevKeys.length !== nextKeys.length) return true;
  for (const key of nextKeys) {
    const before = prev[key] ?? 0;
    const after = next[key] ?? 0;
    // 0 ↔ 非 0 的跨越必须通知，否则低于 epsilon 的尾部衰减不会清掉
    // speaking class，头像会永久留下一圈小绿环。
    if ((before > 0) !== (after > 0)) return true;
    if (Math.abs(before - after) > epsilon) return true;
  }
  return false;
}
