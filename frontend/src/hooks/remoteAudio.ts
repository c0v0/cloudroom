/**
 * 远端音频播放链（P2P 与 SFU 共用）：每个远端流独立播放链
 *   stream → MediaStreamAudioSourceNode → GainNode(每用户音量) → [DynamicsCompressor] → 输出
 * 支持：每用户音量（0-300%，按用户持久记忆）、音量均衡（压缩 + 峰值限制，
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
      if (Number.isFinite(v) && v >= 0 && v <= 3) return v;
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
  src: AudioNode;
  /** 每用户独立压缩器；音量增益放在它之后，避免均衡压缩吃掉手动音量。 */
  compressor: DynamicsCompressorNode;
  gain: GainNode;
  /** 300% 增益后仍限制峰值，避免直接削波。 */
  limiter: DynamicsCompressorNode;
  /** 该用户当前音量（0..3），扬声器静音时增益暂置 0。 */
  volume: number;
  /** 仅元素回退路径存在。 */
  el?: HTMLMediaElement;
  /** 流路径的静音 sink；Chrome 需要媒体元素承接远端轨道，但不从这里取声音。 */
  sink?: HTMLAudioElement;
}

/** 远端音频播放器：每用户独立压缩 + 独立增益（音量始终在压缩之后）。 */
export class RemotePlayer {
  private ctx: AudioContext;
  private chains = new Map<string, Chain>();
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
    this.dynamics = getDynamicsSettings();
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

  private applyCompressorParams(compressor: DynamicsCompressorNode): void {
    // ReaComp 风格预设：阈值可调、比率可调；≥12:1 即峰值限制（ReaLimit 原理）。
    compressor.threshold.value = this.dynamics.threshold;
    compressor.knee.value = 6;
    compressor.ratio.value = this.dynamics.ratio;
    compressor.attack.value = 0.01;
    compressor.release.value = 0.15;
  }

  private connectChain(chain: Chain): void {
    chain.src.disconnect();
    chain.compressor.disconnect();
    chain.limiter.disconnect();
    chain.gain.disconnect();
    if (this.dynamics.enabled) {
      chain.src.connect(chain.compressor);
      chain.compressor.connect(chain.gain);
    } else {
      chain.src.connect(chain.gain);
    }
    chain.gain.connect(chain.limiter);
    chain.limiter.connect(this.ctx.destination);
    if (this.recordDest) chain.limiter.connect(this.recordDest);
  }

  /** 开始本地录音：返回混入所有远端 + 本地麦克风的 MediaStream。 */
  startRecording(localStream?: MediaStream | null): MediaStream | null {
    this.resume();
    if (!this.recordDest) this.recordDest = this.ctx.createMediaStreamDestination();
    for (const ch of this.chains.values()) ch.limiter.connect(this.recordDest);
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
          ch.limiter.disconnect(this.recordDest);
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

  private addChain(
    id: string,
    src: AudioNode,
    el?: HTMLMediaElement,
    sink?: HTMLAudioElement,
  ): void {
    const compressor = this.ctx.createDynamicsCompressor();
    this.applyCompressorParams(compressor);
    const limiter = this.ctx.createDynamicsCompressor();
    limiter.threshold.value = -1;
    limiter.knee.value = 0;
    limiter.ratio.value = 20;
    limiter.attack.value = 0.003;
    limiter.release.value = 0.1;
    const gain = this.ctx.createGain();
    const volume = getVolumePref(id);
    gain.gain.value = this.speakerMuted ? 0 : volume;
    const chain: Chain = { src, compressor, gain, limiter, volume, el, sink };
    this.chains.set(id, chain);
    this.connectChain(chain);
  }

  /**
   * 绑定 WebRTC 音频流。关键：不能走 <audio> + createMediaElementSource，
   * Chrome 对 srcObject=MediaStream 的媒体元素可能不向 WebAudio 输出；
   * 直接使用 MediaStreamAudioSourceNode 才能让音量和均衡真实生效。
   */
  attachStream(id: string, stream: MediaStream): void {
    this.resume();
    if (this.chains.has(id)) this.detach(id);
    this.elements.delete(id);
    let sink: HTMLAudioElement | undefined;
    try {
      // Chrome 需要媒体元素承接远端 WebRTC 轨道，否则 MediaStreamAudioSource
      // 可能拿不到音频；元素保持静音，实际声音只走 WebAudio 增益链。
      sink = document.createElement("audio");
      sink.muted = true;
      sink.autoplay = true;
      sink.setAttribute("playsinline", "");
      sink.style.display = "none";
      sink.srcObject = stream;
      document.body.appendChild(sink);
      void sink.play().catch(() => undefined);
      const src = this.ctx.createMediaStreamSource(stream);
      this.addChain(id, src, undefined, sink);
    } catch (error) {
      sink?.remove();
      console.warn("[audio] MediaStream 播放链初始化失败", error);
    }
  }

  /** 保留元素绑定，供测试和不支持 MediaStreamAudioSourceNode 的旧环境使用。 */
  attach(id: string, el: HTMLMediaElement): void {
    this.resume();
    if (this.chains.has(id)) this.detach(id);
    this.elements.set(id, el);
    el.muted = this.speakerMuted;
    el.volume = this.speakerMuted ? 0 : 1;
    try {
      let src = this.sources.get(el);
      if (!src) {
        src = this.ctx.createMediaElementSource(el);
        this.sources.set(el, src);
      }
      el.volume = this.speakerMuted ? 0 : 1;
      this.addChain(id, src, el);
    } catch (error) {
      // 元素回退路径：原生音量至少还能调小/静音。
      el.volume = this.speakerMuted ? 0 : getVolumePref(id);
      console.warn("[audio] WebAudio 播放链初始化失败，已回退原生播放", error);
    }
  }

  /** 调整某用户音量（0..3 = 0%..300%）。 */
  setVolume(id: string, v: number): void {
    setVolumePref(id, v);
    const ch = this.chains.get(id);
    if (ch) {
      ch.volume = v;
      ch.gain.gain.value = this.speakerMuted ? 0 : v;
    } else {
      // WebAudio 回退路径：没有 gain 链时至少调整原生元素音量。
      const el = this.elements.get(id);
      if (el) el.volume = this.speakerMuted ? 0 : v;
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
    for (const ch of this.chains.values()) {
      this.applyCompressorParams(ch.compressor);
      this.connectChain(ch);
    }
  }

  detach(id: string): void {
    this.elements.delete(id);
    const ch = this.chains.get(id);
    if (ch) {
      ch.src.disconnect();
      ch.compressor.disconnect();
      ch.limiter.disconnect();
      ch.gain.disconnect();
      if (ch.sink) {
        ch.sink.pause();
        ch.sink.srcObject = null;
        ch.sink.remove();
      }
      this.chains.delete(id);
    }
  }

  close(): void {
    for (const id of [...this.chains.keys()]) this.detach(id);
    this.elements.clear();
    void this.ctx.close();
  }
}
