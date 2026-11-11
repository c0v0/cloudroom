/**
 * 远端音频播放链（P2P 与 SFU 共用）：每个远端流独立播放链
 *   stream → MediaStreamAudioSourceNode → GainNode(每用户音量) → [DynamicsCompressor] → 输出
 * 支持：每用户音量（0-200%，按用户持久记忆）、音量均衡（压缩 + 峰值限制，
 * 参数可调）、扬声器静音（master 静音保留每用户音量值）。
 */

export interface DynamicsSettings {
  /** 音量均衡开关（压缩 + 峰值限制）。 */
  enabled: boolean;
  /** 压缩阈值（dB，-60..0）。 */
  threshold: number;
  /** 压缩比率（1..20；≥12 兼作峰值限制）。 */
  ratio: number;
  /** 关闭静音压缩（DTX）：音乐/持续环境声场景防误判静音。 */
  dtxOff: boolean;
}

const VOL_KEY = "cloudroom.volumes";
const DYN_KEY = "cloudroom.dynamics";

// ---------------------------------------------------------- 偏好与订阅

type Listener = () => void;
const dynListeners = new Set<Listener>();

export function subscribeDynamics(listener: Listener): () => void {
  dynListeners.add(listener);
  return () => {
    dynListeners.delete(listener);
  };
}

export function getVolumePref(userId: string): number {
  try {
    const raw = localStorage.getItem(VOL_KEY);
    if (raw) {
      const v = Number(JSON.parse(raw)[userId]);
      if (Number.isFinite(v) && v >= 0 && v <= 2) return v;
    }
  } catch {
    /* 忽略损坏数据 */
  }
  return 1;
}

export function setVolumePref(userId: string, v: number): void {
  try {
    const raw = localStorage.getItem(VOL_KEY);
    const all = raw ? (JSON.parse(raw) as Record<string, number>) : {};
    all[userId] = v;
    localStorage.setItem(VOL_KEY, JSON.stringify(all));
  } catch {
    /* 隐私模式等场景忽略 */
  }
}

export function getDynamicsSettings(): DynamicsSettings {
  try {
    const raw = localStorage.getItem(DYN_KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<DynamicsSettings>;
      return {
        enabled: v.enabled === true,
        threshold: Number.isFinite(v.threshold) ? Math.max(-60, Math.min(0, v.threshold!)) : -24,
        ratio: Number.isFinite(v.ratio) ? Math.max(1, Math.min(20, v.ratio!)) : 4,
        dtxOff: v.dtxOff === true,
      };
    }
  } catch {
    /* 忽略损坏数据 */
  }
  return { enabled: false, threshold: -24, ratio: 4, dtxOff: false };
}

export function setDynamicsPrefs(prefs: Partial<DynamicsSettings>): void {
  try {
    const cur = getDynamicsSettings();
    localStorage.setItem(DYN_KEY, JSON.stringify({ ...cur, ...prefs }));
  } catch {
    /* 忽略 */
  }
  dynListeners.forEach((l) => l());
}

// ---------------------------------------------------------- 播放链

interface Chain {
  src: MediaElementAudioSourceNode;
  gain: GainNode;
  /** 该用户当前音量（0..2），扬声器静音时增益暂置 0。 */
  volume: number;
  /** 原始媒体元素；移动端 AudioContext 不可用时仍可原生静音。 */
  el: HTMLMediaElement;
}

/** 远端音频播放器：每用户独立增益 + 全局压缩链（类似 ReaComp/ReaLimit 组合）。 */
export class RemotePlayer {
  private ctx: AudioContext;
  private chains = new Map<string, Chain>();
  private compressor: DynamicsCompressorNode;
  private dynamics: DynamicsSettings;
  /** 每个元素只能创建一次 MediaElementAudioSourceNode，按元素缓存复用。 */
  private sources = new WeakMap<HTMLMediaElement, MediaElementAudioSourceNode>();
  /** 所有已绑定的媒体元素，用于扬声器静音的移动端兜底。 */
  private elements = new Map<string, HTMLMediaElement>();
  private speakerMuted = false;
  /** 本地录音混音目标：开启后所有远端增益与本地麦克风都汇入这里。 */
  private recordDest: MediaStreamAudioDestinationNode | null = null;
  private recordLocal: MediaStreamAudioSourceNode | null = null;

  constructor() {
    this.ctx = new AudioContext();
    this.compressor = this.ctx.createDynamicsCompressor();
    // 压缩器必须接到 destination；否则开启“音量均衡”时远端音频没有输出。
    this.compressor.connect(this.ctx.destination);
    this.dynamics = getDynamicsSettings();
    this.applyCompressorParams();
    this.resume();
    this.applySavedSink();
    if (typeof document !== "undefined") {
      document.addEventListener("pointerdown", this.resume.bind(this), { once: true });
      document.addEventListener("keydown", this.resume.bind(this), { once: true });
    }
  }

  /** 切换扬声器（Chrome 110+ 的 AudioContext.setSinkId）；不支持时静默忽略。 */
  setOutputDevice(deviceId: string): void {
    const ctx = this.ctx as AudioContext & { setSinkId?: (id: string) => Promise<void> };
    if (typeof ctx.setSinkId !== "function") return;
    void ctx.setSinkId(deviceId || "").catch(() => undefined);
  }

  private applySavedSink(): void {
    let id = "";
    try {
      id = localStorage.getItem("cloudroom.spkId") || "";
    } catch {
      /* 隐私模式忽略 */
    }
    if (id) this.setOutputDevice(id);
  }
  /** 移动端/自动播放策略：在用户手势或绑定流时恢复 AudioContext。 */
  resume(): void {
    if (this.ctx.state === "suspended") void this.ctx.resume();
  }

  private applyCompressorParams(): void {
    // ReaComp 风格预设：阈值可调、比率可调；≥12:1 即峰值限制（ReaLimit 原理）。
    const c = this.compressor;
    c.threshold.value = this.dynamics.threshold;
    c.knee.value = 6;
    c.ratio.value = this.dynamics.ratio;
    c.attack.value = 0.01;
    c.release.value = 0.15;
  }

  private connectOut(gain: GainNode): void {
    gain.disconnect();
    if (this.dynamics.enabled) {
      gain.connect(this.compressor);
    } else {
      gain.connect(this.ctx.destination);
    }
  }

  /** 开始本地录音：返回混入所有远端 + 本地麦克风的 MediaStream。 */
  startRecording(localStream?: MediaStream | null): MediaStream | null {
    this.resume();
    if (!this.recordDest) this.recordDest = this.ctx.createMediaStreamDestination();
    for (const ch of this.chains.values()) ch.gain.connect(this.recordDest);
    if (localStream && !this.recordLocal) {
      this.recordLocal = this.ctx.createMediaStreamSource(localStream);
      this.recordLocal.connect(this.recordDest);
    }
    return this.recordDest.stream;
  }

  /** 停止录音并断开混音节点。 */
  stopRecording(): void {
    if (this.recordDest) {
      for (const ch of this.chains.values()) {
        try {
          ch.gain.disconnect(this.recordDest);
        } catch {
          /* 已断开 */
        }
      }
    }
    try {
      this.recordLocal?.disconnect();
    } catch {
      /* 已断开 */
    }
    this.recordLocal = null;
    this.recordDest = null;
  }

  /** 绑定远端播放元素（重复绑定先解绑旧链）。元素负责解码，WebAudio 只做增益/压缩。 */
  attach(id: string, el: HTMLMediaElement): void {
    this.resume();
    if (this.chains.has(id)) this.detach(id);
    this.elements.set(id, el);
    // 元素静音/音量为移动端兜底：部分浏览器会挂起/绕过 AudioContext 增益链。
    el.muted = this.speakerMuted;
    el.volume = this.speakerMuted ? 0 : 1;
    try {
      let src = this.sources.get(el);
      if (!src) {
        src = this.ctx.createMediaElementSource(el);
        this.sources.set(el, src);
      }
      // 音量统一由 GainNode 控制，元素本身保持 1 倍，避免双重衰减；静音时用 0 兜底。
      el.volume = this.speakerMuted ? 0 : 1;
      const gain = this.ctx.createGain();
      // 应用该用户的音量偏好（扬声器静音时增益暂置 0）。
      const volume = getVolumePref(id);
      gain.gain.value = this.speakerMuted ? 0 : volume;
      src.connect(gain);
      this.connectOut(gain);
      this.chains.set(id, { src, gain, volume, el });
      if (this.recordDest) gain.connect(this.recordDest);
    } catch {
      // WebAudio 不可用时保留原生 <audio> 播放，静音仍由 el.muted 生效。
    }
  }

  /** 调整某用户音量（0..2 = 0%..200%）。 */
  setVolume(id: string, v: number): void {
    setVolumePref(id, v);
    const ch = this.chains.get(id);
    if (ch) {
      ch.volume = v;
      ch.gain.gain.value = this.speakerMuted ? 0 : v;
    }
  }

  /** 扬声器静音（保留每用户音量值，恢复时还原）。 */
  setSpeakerMuted(muted: boolean): void {
    this.resume();
    this.speakerMuted = muted;
    for (const el of this.elements.values()) {
      el.muted = muted;
      el.volume = muted ? 0 : 1;
    }
    for (const ch of this.chains.values()) {
      ch.gain.gain.value = muted ? 0 : ch.volume;
    }
  }

  /** 更新音量均衡设置（开关/参数），重连播放路径。 */
  setDynamics(d: DynamicsSettings): void {
    this.dynamics = d;
    this.applyCompressorParams();
    for (const ch of this.chains.values()) {
      this.connectOut(ch.gain);
    }
  }

  detach(id: string): void {
    this.elements.delete(id);
    const ch = this.chains.get(id);
    if (ch) {
      ch.src.disconnect();
      ch.gain.disconnect();
      this.chains.delete(id);
    }
  }

  close(): void {
    for (const id of [...this.chains.keys()]) this.detach(id);
    this.elements.clear();
    void this.ctx.close();
  }
}
