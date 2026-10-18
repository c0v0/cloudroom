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
  src: MediaStreamAudioSourceNode;
  gain: GainNode;
  /** 该用户当前音量（0..2），扬声器静音时增益暂置 0。 */
  volume: number;
}

/** 远端音频播放器：每用户独立增益 + 全局压缩链（类似 ReaComp/ReaLimit 组合）。 */
export class RemotePlayer {
  private ctx: AudioContext;
  private chains = new Map<string, Chain>();
  private compressor: DynamicsCompressorNode;
  private dynamics: DynamicsSettings;
  private speakerMuted = false;

  constructor() {
    this.ctx = new AudioContext();
    this.compressor = this.ctx.createDynamicsCompressor();
    this.dynamics = getDynamicsSettings();
    this.applyCompressorParams();
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

  /** 绑定远端流（重复绑定先解绑旧源）。 */
  attach(id: string, stream: MediaStream): void {
    if (this.chains.has(id)) this.detach(id);
    const src = this.ctx.createMediaStreamSource(stream);
    const gain = this.ctx.createGain();
    // 应用该用户的音量偏好（扬声器静音时增益暂置 0）。
    const volume = getVolumePref(id);
    gain.gain.value = this.speakerMuted ? 0 : volume;
    src.connect(gain);
    this.connectOut(gain);
    this.chains.set(id, { src, gain, volume });
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
    this.speakerMuted = muted;
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
    const ch = this.chains.get(id);
    if (ch) {
      ch.src.disconnect();
      ch.gain.disconnect();
      this.chains.delete(id);
    }
  }

  close(): void {
    for (const id of [...this.chains.keys()]) this.detach(id);
    void this.ctx.close();
  }
}
