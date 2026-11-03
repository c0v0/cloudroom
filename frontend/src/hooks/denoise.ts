/**
 * 用户侧声音设置（降噪 / 回声消除 / 自动增益）与 rnnoise WASM 降噪引擎。
 * 设置存 localStorage，属个人偏好；通过 subscribeNoise 跨组件即时生效
 * （设置模态框 → 语音 hook / 麦克风测试），语音中修改自动重建媒体流。
 */
import { createRNNWasmModule } from "@jitsi/rnnoise-wasm";
import rnnoiseWasmUrl from "@jitsi/rnnoise-wasm/dist/rnnoise.wasm?url";

export type NoiseMode = "off" | "webrtc" | "rnnoise";

export interface NoiseSettings {
  mode: NoiseMode;
  /** 回声消除（双讲削波/误杀第三方音频场景可关闭）。 */
  aec: boolean;
  /** 自动增益（环境噪声放大/音乐动态压缩场景可关闭）。 */
  agc: boolean;
}

const KEY = "cloudroom.noise";
const FRAME = 480; // rnnoise 帧长（48kHz 10ms）

// ---------------------------------------------------------- 设置与订阅

type Listener = () => void;
const listeners = new Set<Listener>();

/** 订阅声音设置变化（设置模态框 → 语音 hook / 麦克风测试）。 */
export function subscribeNoise(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function getNoiseSettings(): NoiseSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<NoiseSettings>;
      if (v.mode === "off" || v.mode === "webrtc" || v.mode === "rnnoise") {
        return {
          mode: v.mode,
          aec: v.aec !== false,
          agc: v.agc !== false,
        };
      }
    }
  } catch {
    /* 忽略损坏的本地设置 */
  }
  return { mode: "webrtc", aec: true, agc: true };
}

export function setNoisePrefs(prefs: Partial<NoiseSettings>): void {
  try {
    const cur = getNoiseSettings();
    localStorage.setItem(KEY, JSON.stringify({ ...cur, ...prefs }));
  } catch {
    /* 隐私模式等场景忽略 */
  }
  listeners.forEach((l) => l());
}

// ------------------------------------------------------- rnnoise WASM

/** rnnoise WASM 的 emscripten 模块形状（C 导出子集）。 */
export interface RnnModule {
  _rnnoise_create(): number;
  _rnnoise_process_frame(state: number, inPtr: number, outPtr: number): void;
  _rnnoise_destroy(state: number): void;
  _malloc(bytes: number): number;
  _free(ptr: number): void;
  HEAPF32: Float32Array;
}

class RnnProcessor {
  private statePtr: number;
  private inPtr: number;
  private outPtr: number;

  private constructor(private module: RnnModule) {
    this.statePtr = module._rnnoise_create();
    this.inPtr = module._malloc(FRAME * 4);
    this.outPtr = module._malloc(FRAME * 4);
  }

  static async create(): Promise<RnnProcessor> {
    const module = (await createRNNWasmModule({
      locateFile: () => rnnoiseWasmUrl,
    })) as unknown as RnnModule;
    // emscripten 异步加载完成（wasm fetch + 实例化）。
    await new Promise<void>((resolve) => {
      const check = () => {
        if (typeof module._rnnoise_create === "function" && module.HEAPF32) resolve();
        else setTimeout(check, 5);
      };
      check();
    });
    return new RnnProcessor(module);
  }

  /** 就地处理一帧（Float32Array(480)），返回同一数组。 */
  process(frame: Float32Array): Float32Array {
    this.module.HEAPF32.set(frame, this.inPtr / 4);
    this.module._rnnoise_process_frame(this.statePtr, this.inPtr, this.outPtr);
    frame.set(this.module.HEAPF32.subarray(this.outPtr / 4, this.outPtr / 4 + FRAME));
    return frame;
  }

  destroy(): void {
    this.module._rnnoise_destroy(this.statePtr);
    this.module._free(this.inPtr);
    this.module._free(this.outPtr);
  }
}

/** AudioWorklet 源码（Blob 内联，避免打包器对 worklet 模块的处理差异）。 */
const WORKLET_CODE = `
const FRAME = 480;
let buf = new Float32Array(FRAME);
let pos = 0;
// rnnoise 每次处理 480 帧，而 Web Audio 每个渲染量子只有 128 帧；
// 必须用待输出队列跨量子搬运，直接 set(480) 会抛 RangeError 并静音。
let queue = new Float32Array(0);
class RnnWorklet extends AudioWorkletProcessor {
  constructor() {
    super();
    this.port.onmessage = (e) => {
      const d = e.data;
      if (!(d instanceof Float32Array) || d.length === 0) return;
      if (queue.length === 0) {
        queue = d;
      } else {
        const merged = new Float32Array(queue.length + d.length);
        merged.set(queue, 0);
        merged.set(d, queue.length);
        queue = merged;
      }
    };
  }
  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (!input || !output) return true;
    for (let i = 0; i < input.length; i++) {
      buf[pos++] = input[i];
      if (pos === FRAME) {
        this.port.postMessage(buf);
        buf = new Float32Array(FRAME);
        pos = 0;
      }
    }
    let filled = 0;
    while (filled < output.length && queue.length > 0) {
      const n = Math.min(output.length - filled, queue.length);
      output.set(queue.subarray(0, n), filled);
      filled += n;
      queue = queue.subarray(n);
    }
    if (filled < output.length) output.fill(0, filled);
    return true;
  }
}
registerProcessor("rnnoise-processor", RnnWorklet);
`;

/**
 * rnnoise 处理管线：输入原始流，输出降噪后流。
 * AudioContext 强制 48kHz（rnnoise 只支持 48k）；设备不支持时 create 抛错，
 * 由调用方降级为 webrtc 模式。
 */
export class DenoiseEngine {
  private input: MediaStreamAudioSourceNode | null = null;

  private constructor(
    private ctx: AudioContext,
    private worklet: AudioWorkletNode,
    private dest: MediaStreamAudioDestinationNode,
    private rnn: RnnProcessor,
  ) {}

  static async create(): Promise<DenoiseEngine> {
    const ctx = new AudioContext({ sampleRate: 48000 });
    if (Math.round(ctx.sampleRate) !== 48000) {
      void ctx.close();
      throw new Error("设备不支持 48kHz 采样率，无法使用 rnnoise 降噪");
    }
    const blobUrl = URL.createObjectURL(new Blob([WORKLET_CODE], { type: "application/javascript" }));
    try {
      await ctx.audioWorklet.addModule(blobUrl);
    } finally {
      URL.revokeObjectURL(blobUrl);
    }
    const rnn = await RnnProcessor.create();
    const dest = ctx.createMediaStreamDestination();
    const worklet = new AudioWorkletNode(ctx, "rnnoise-processor");
    worklet.port.onmessage = (e) => {
      worklet.port.postMessage(rnn.process(e.data));
    };
    worklet.connect(dest);
    return new DenoiseEngine(ctx, worklet, dest, rnn);
  }

  /** 绑定输入流（重复调用先解绑）。 */
  attach(input: MediaStream): void {
    this.input?.disconnect();
    this.input = this.ctx.createMediaStreamSource(input);
    this.input.connect(this.worklet);
  }

  get stream(): MediaStream {
    return this.dest.stream;
  }

  close(): void {
    this.input?.disconnect();
    this.rnn.destroy();
    void this.ctx.close();
  }
}
