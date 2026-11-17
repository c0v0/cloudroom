/**
 * 麦克风发送门限：低于阈值的输入平滑降到 0，避免把环境底噪持续发给远端。
 * 动画用的 VAD 独立，不在这里处理。
 */
const GATE_KEY = "cloudroom.gateDb";
const GATE_HOLD_KEY = "cloudroom.gateHoldMs";
type Listener = () => void;
const listeners = new Set<Listener>();

function readGate(): number | null {
  try {
    const raw = localStorage.getItem(GATE_KEY);
    if (raw === null || raw === "") return null;
    const db = Number(raw);
    if (!Number.isFinite(db) || db < -60 || db > -20) return null;
    return db;
  } catch {
    return null;
  }
}
export function getGateDb(): number | null {
  return readGate();
}
export function getGateHoldMs(): number {
  try {
    const raw = localStorage.getItem(GATE_HOLD_KEY);
    const ms = raw === null ? 250 : Number(raw);
    if (!Number.isFinite(ms)) return 250;
    return Math.max(0, Math.min(1000, Math.round(ms)));
  } catch {
    return 250;
  }
}
export function setGateHoldMs(ms: number): void {
  try {
    localStorage.setItem(GATE_HOLD_KEY, String(Math.max(0, Math.min(1000, Math.round(ms)))));
  } catch {
    /* 隐私模式忽略 */
  }
  for (const fn of listeners) fn();
}
export function setGateDb(db: number | null): void {
  try {
    if (db === null) localStorage.removeItem(GATE_KEY);
    else localStorage.setItem(GATE_KEY, String(Math.max(-60, Math.min(-20, db))));
  } catch {
    /* 隐私模式忽略 */
  }
  for (const fn of listeners) fn();
}
export function subscribeGate(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

const HYSTERESIS_DB = 6;

/** 软门限：输入 → 分析器 + GainNode → 输出 MediaStream。 */
export class MicGate {
  private ctx: AudioContext;
  private source: MediaStreamAudioSourceNode;
  private analyser: AnalyserNode;
  private gain: GainNode;
  private dest: MediaStreamAudioDestinationNode;
  private input: MediaStream;
  private timer: number;
  private buf: Float32Array<ArrayBuffer>;
  private open = false;
  private lastVoiceAt = 0;
  /** PTT 按住时绕过门限：用户已明确表示要发送。 */
  private bypass = false;

  constructor(input: MediaStream) {
    this.input = input;
    this.ctx = new AudioContext();
    this.source = this.ctx.createMediaStreamSource(input);
    this.analyser = this.ctx.createAnalyser();
    this.analyser.fftSize = 1024;
    this.buf = new Float32Array(this.analyser.fftSize);
    this.gain = this.ctx.createGain();
    this.gain.gain.value = 0;
    this.dest = this.ctx.createMediaStreamDestination();
    this.source.connect(this.analyser);
    this.source.connect(this.gain);
    this.gain.connect(this.dest);
    void this.ctx.resume();
    this.timer = window.setInterval(() => this.tick(), 50);
  }

  get stream(): MediaStream {
    return this.dest.stream;
  }

  setBypass(bypass: boolean): void {
    this.bypass = bypass;
    if (bypass) {
      this.open = true;
      this.lastVoiceAt = Date.now();
    }
  }

  private tick(): void {
    const threshold = getGateDb();
    if (this.bypass) {
      this.gain.gain.setTargetAtTime(1, this.ctx.currentTime, 0.015);
      return;
    }
    if (threshold === null) {
      this.gain.gain.setTargetAtTime(1, this.ctx.currentTime, 0.02);
      this.open = true;
      return;
    }
    this.analyser.getFloatTimeDomainData(this.buf);
    let sum = 0;
    for (let i = 0; i < this.buf.length; i++) sum += this.buf[i] * this.buf[i];
    const rms = Math.sqrt(sum / this.buf.length);
    const db = rms > 1e-7 ? 20 * Math.log10(rms) : -100;
    const closeAt = threshold - HYSTERESIS_DB;
    const now = Date.now();
    if (db >= (this.open ? closeAt : threshold)) {
      this.open = true;
      this.lastVoiceAt = now;
    } else if (this.open && now - this.lastVoiceAt > getGateHoldMs()) {
      this.open = false;
    }
    this.gain.gain.setTargetAtTime(
      this.open ? 1 : 0,
      this.ctx.currentTime,
      this.open ? 0.015 : 0.08,
    );
  }

  close(): void {
    window.clearInterval(this.timer);
    this.source.disconnect();
    this.analyser.disconnect();
    this.gain.disconnect();
    this.input.getTracks().forEach((t) => t.stop());
    void this.ctx.close().catch(() => undefined);
  }
}
