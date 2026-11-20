/**
 * 用户侧声音设置（降噪 / 回声消除 / 自动增益）与免费 WASM 降噪引擎。
 * RNNoise/GTCRN/Speex 使用 @sapphi-red/web-noise-suppressor，
 * DTLN/DeepFilterNet3 使用 @workadventure/noise-suppression。
 * 设置存 localStorage，属个人偏好；通过 subscribeNoise 跨组件即时生效
 * （设置模态框 → 语音 hook / 麦克风测试），语音中修改自动重建媒体流。
 */

export type NoiseMode =
  | "off"
  | "webrtc"
  | "rnnoise"
  | "gtcrn"
  | "dtln"
  | "deepfilternet3"
  | "speex";

/** 需要 AudioWorklet / WASM 处理的降噪模式。 */
export type WasmNoiseMode = Exclude<NoiseMode, "off" | "webrtc">;

export interface PostGainParams {
  /** 目标语音电平（dBFS，10ms 帧 RMS）。 */
  targetDb: number;
  /** 最大可增加增益。 */
  maxGainDb: number;
  /** 最大可降低增益。 */
  minGainDb: number;
  /** 低于此电平的帧不视为语音，不推动增益。 */
  speechThresholdDb: number;
  /** 每帧增益调整速度。 */
  adaptDbPerFrame: number;
  /** 后置增益后的峰值上限（线性幅度）。 */
  ceiling: number;
}

export interface DeepFilterNetParams {
  /** 说话时最大衰减，100 会把字间背景压到静音。 */
  speechAttenuationDb: number;
  /** 停顿期间衰减；小于等于 speechAttenuationDb 时关闭停顿门限。 */
  pauseAttenuationDb: number;
  /** 停顿门限前视帧数（10ms/帧），越大越不容易削字头。 */
  pauseGateLookaheadFrames: number;
  /** 打开停顿门限所需的连续语音帧数。 */
  minSpeechFrames: number;
  /** 模型后置增益，替代浏览器 AGC。 */
  postGain: boolean;
  /** 后置增益的细分参数，仅在 postGain 开启时生效。 */
  postGainOptions: PostGainParams;
  /** 允许的实时 CPU 负载比例；0 关闭检查。 */
  maxLoad: number;
  /** 模型加载期间直通麦克风。 */
  bypassUntilReady: boolean;
  /** 等待模型就绪的超时（毫秒）。 */
  readyTimeoutMs: number;
}

export interface DtlnParams {
  /** 模型加载期间直通麦克风。 */
  bypassUntilReady: boolean;
  /** 等待模型就绪的超时（毫秒）。 */
  readyTimeoutMs: number;
}

export interface NoiseParams {
  deepfilternet3: DeepFilterNetParams;
  dtln: DtlnParams;
}

export interface NoiseSettings {
  mode: NoiseMode;
  /** 回声消除（双讲削波/误杀第三方音频场景可关闭）。 */
  aec: boolean;
  /** 自动增益（环境噪声放大/音乐动态压缩场景可关闭）。 */
  agc: boolean;
  /** 各降噪模型的高级参数。 */
  params: NoiseParams;
}

export const DEFAULT_POST_GAIN_PARAMS: PostGainParams = {
  targetDb: -23,
  maxGainDb: 15,
  minGainDb: -10,
  speechThresholdDb: -45,
  adaptDbPerFrame: 0.05,
  ceiling: 0.9,
};

export const DEFAULT_DEEPFILTERNET3_PARAMS: DeepFilterNetParams = {
  speechAttenuationDb: 25,
  pauseAttenuationDb: 45,
  pauseGateLookaheadFrames: 3,
  minSpeechFrames: 1,
  postGain: false,
  postGainOptions: DEFAULT_POST_GAIN_PARAMS,
  maxLoad: 0.7,
  bypassUntilReady: true,
  readyTimeoutMs: 30000,
};

export const DEFAULT_DTLN_PARAMS: DtlnParams = {
  bypassUntilReady: true,
  readyTimeoutMs: 30000,
};

export const DEFAULT_NOISE_PARAMS: NoiseParams = {
  deepfilternet3: DEFAULT_DEEPFILTERNET3_PARAMS,
  dtln: DEFAULT_DTLN_PARAMS,
};

export interface NoiseProviderInfo {
  id: NoiseMode;
  label: string;
  summary: string;
  /** 采样率；0 表示由浏览器决定。 */
  sampleRate: number;
}

/** 设置页和网络状态展示共用的模型元数据。 */
export const NOISE_PROVIDERS: readonly NoiseProviderInfo[] = [
  {
    id: "off",
    label: "关闭",
    summary: "不处理，直接发送原声",
    sampleRate: 0,
  },
  {
    id: "webrtc",
    label: "WebRTC",
    summary: "浏览器自带 · 推荐",
    sampleRate: 0,
  },
  {
    id: "rnnoise",
    label: "RNNoise",
    summary: "轻量，占用低",
    sampleRate: 48000,
  },
  {
    id: "gtcrn",
    label: "GTCRN",
    summary: "降噪更好，占用较低",
    sampleRate: 48000,
  },
  {
    id: "dtln",
    label: "DTLN",
    summary: "低功耗，音质偏闷",
    sampleRate: 16000,
  },
  {
    id: "deepfilternet3",
    label: "DeepFilterNet3",
    summary: "高质量，占用较高",
    sampleRate: 48000,
  },
  {
    id: "speex",
    label: "Speex",
    summary: "传统降噪，占用低",
    sampleRate: 48000,
  },
];

const NOISE_MODES: Record<NoiseMode, true> = {
  off: true,
  webrtc: true,
  rnnoise: true,
  gtcrn: true,
  dtln: true,
  deepfilternet3: true,
  speex: true,
};

export function isNoiseMode(value: unknown): value is NoiseMode {
  return (
    typeof value === "string" &&
    Object.prototype.hasOwnProperty.call(NOISE_MODES, value)
  );
}

export function isWasmNoiseMode(value: NoiseMode): value is WasmNoiseMode {
  return value !== "off" && value !== "webrtc";
}

export function noiseLabel(mode: NoiseMode): string {
  return NOISE_PROVIDERS.find((p) => p.id === mode)?.label ?? mode;
}

/** 把浏览器/模型的原始错误转成用户能处理的提示。 */
export function noiseErrorMessage(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  if (/NotAllowed|PermissionDenied|permission/i.test(text)) {
    return "麦克风权限被拒绝，请在浏览器中允许麦克风后重试";
  }
  if (/NotFound|Requested device not found|not found/i.test(text)) {
    return "没有找到麦克风，请检查设备连接";
  }
  return text;
}

export function noiseProvider(mode: NoiseMode): NoiseProviderInfo {
  return NOISE_PROVIDERS.find((p) => p.id === mode) ?? NOISE_PROVIDERS[1];
}

const KEY = "cloudroom.noise";

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

function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

function readPostGainParams(raw: unknown): PostGainParams {
  const v = raw && typeof raw === "object" ? (raw as Partial<PostGainParams>) : {};
  const fallback = DEFAULT_POST_GAIN_PARAMS;
  return {
    targetDb: clamp(v.targetDb ?? fallback.targetDb, -60, 0),
    maxGainDb: clamp(v.maxGainDb ?? fallback.maxGainDb, 0, 40),
    minGainDb: clamp(v.minGainDb ?? fallback.minGainDb, -40, 0),
    speechThresholdDb: clamp(
      v.speechThresholdDb ?? fallback.speechThresholdDb,
      -80,
      0,
    ),
    adaptDbPerFrame: clamp(
      v.adaptDbPerFrame ?? fallback.adaptDbPerFrame,
      0.001,
      1,
    ),
    ceiling: clamp(v.ceiling ?? fallback.ceiling, 0.1, 1),
  };
}

function readDeepFilterNetParams(raw: unknown): DeepFilterNetParams {
  const v = raw && typeof raw === "object" ? (raw as Partial<DeepFilterNetParams>) : {};
  const fallback = DEFAULT_DEEPFILTERNET3_PARAMS;
  return {
    speechAttenuationDb: clamp(
      v.speechAttenuationDb ?? fallback.speechAttenuationDb,
      0,
      100,
    ),
    pauseAttenuationDb: clamp(
      v.pauseAttenuationDb ?? fallback.pauseAttenuationDb,
      0,
      100,
    ),
    pauseGateLookaheadFrames: Math.round(
      clamp(v.pauseGateLookaheadFrames ?? fallback.pauseGateLookaheadFrames, 0, 10),
    ),
    minSpeechFrames: Math.round(
      clamp(v.minSpeechFrames ?? fallback.minSpeechFrames, 1, 5),
    ),
    postGain: v.postGain === true,
    postGainOptions: readPostGainParams(v.postGainOptions),
    maxLoad: clamp(v.maxLoad ?? fallback.maxLoad, 0, 1),
    bypassUntilReady: v.bypassUntilReady !== false,
    readyTimeoutMs: Math.round(
      clamp(v.readyTimeoutMs ?? fallback.readyTimeoutMs, 1000, 60000),
    ),
  };
}

function readDtlnParams(raw: unknown): DtlnParams {
  const v = raw && typeof raw === "object" ? (raw as Partial<DtlnParams>) : {};
  const fallback = DEFAULT_DTLN_PARAMS;
  return {
    bypassUntilReady: v.bypassUntilReady !== false,
    readyTimeoutMs: Math.round(
      clamp(v.readyTimeoutMs ?? fallback.readyTimeoutMs, 1000, 60000),
    ),
  };
}

function readParams(raw: unknown): NoiseParams {
  const v = raw && typeof raw === "object" ? (raw as Partial<NoiseParams>) : {};
  return {
    deepfilternet3: readDeepFilterNetParams(v.deepfilternet3),
    dtln: readDtlnParams(v.dtln),
  };
}

export function getNoiseSettings(): NoiseSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const v = JSON.parse(raw) as Partial<NoiseSettings>;
      if (isNoiseMode(v.mode)) {
        return {
          mode: v.mode,
          aec: v.aec !== false,
          agc: v.agc !== false,
          params: readParams(v.params),
        };
      }
    }
  } catch {
    /* 忽略损坏的本地设置 */
  }
  return {
    mode: "webrtc",
    aec: true,
    agc: true,
    params: readParams(DEFAULT_NOISE_PARAMS),
  };
}

export function setNoisePrefs(prefs: Partial<NoiseSettings>): void {
  try {
    const cur = getNoiseSettings();
    const next: NoiseSettings = {
      ...cur,
      ...prefs,
      params: prefs.params
        ? {
            deepfilternet3: { ...cur.params.deepfilternet3, ...prefs.params.deepfilternet3 },
            dtln: { ...cur.params.dtln, ...prefs.params.dtln },
          }
        : cur.params,
    };
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    /* 隐私模式等场景忽略 */
  }
  listeners.forEach((l) => l());
}

export function noiseParamsKey(settings: NoiseSettings): string {
  return JSON.stringify(settings.params);
}

// ------------------------------------------------------ 运行时状态

export type NoiseRuntimeState = "idle" | "loading" | "ready" | "fallback" | "error";

export interface NoiseRuntimeStatus {
  requested: NoiseMode;
  actual: NoiseMode;
  state: NoiseRuntimeState;
  message: string;
}

let runtimeStatus: NoiseRuntimeStatus = {
  requested: "off",
  actual: "off",
  state: "idle",
  message: "",
};
const runtimeListeners = new Set<Listener>();

export function getNoiseRuntimeStatus(): NoiseRuntimeStatus {
  return runtimeStatus;
}

export function setNoiseRuntimeStatus(status: NoiseRuntimeStatus): void {
  runtimeStatus = status;
  runtimeListeners.forEach((l) => l());
}

export function subscribeNoiseRuntime(listener: Listener): () => void {
  runtimeListeners.add(listener);
  return () => {
    runtimeListeners.delete(listener);
  };
}

/** 旧采集/设备默认可能给双声道；模型按单声道建模，显式收敛避免右声道静音。 */
function configureMono(node: AudioNode, dest: MediaStreamAudioDestinationNode): void {
  node.channelCount = 1;
  node.channelCountMode = "explicit";
  node.channelInterpretation = "speakers";
  dest.channelCount = 1;
  dest.channelCountMode = "explicit";
}

/** 所有降噪引擎共用的生命周期；调用方只持有处理后流。 */
export interface NoiseEngine {
  readonly mode: WasmNoiseMode;
  readonly stream: MediaStream;
  attach(input: MediaStream): void;
  /** 在同一个 AudioContext 内建立直通参考，用于抵消上下文/MediaStream 基线。 */
  createBypass(input: MediaStream): MediaStream;
  close(): void;
}

// ------------------------------------------------- 免费 WASM 包引擎

type SimpleWorkletMode = "rnnoise" | "gtcrn" | "speex";

/** @sapphi-red/web-noise-suppressor 的 RNNoise/GTCRN/Speex 节点。 */
class SimpleWorkletEngine implements NoiseEngine {
  private input: MediaStreamAudioSourceNode | null = null;
  private source: MediaStream | null = null;

  private constructor(
    readonly mode: SimpleWorkletMode,
    private ctx: AudioContext,
    private node: AudioWorkletNode & { destroy?: () => void },
    private dest: MediaStreamAudioDestinationNode,
  ) {}

  static async create(mode: SimpleWorkletMode): Promise<SimpleWorkletEngine> {
    const pkg = await import("@sapphi-red/web-noise-suppressor");
    const ctx = new AudioContext({ sampleRate: 48000 });
    await ctx.resume();
    if (Math.round(ctx.sampleRate) !== 48000) {
      void ctx.close();
      throw new Error("当前设备不支持 48 kHz 音频，无法使用这个降噪模型");
    }

    let node: AudioWorkletNode & { destroy?: () => void };
    if (mode === "rnnoise") {
      const [worklet, wasm, simd] = await Promise.all([
        import("@sapphi-red/web-noise-suppressor/rnnoiseWorklet.js?url"),
        import("@sapphi-red/web-noise-suppressor/rnnoise.wasm?url"),
        import("@sapphi-red/web-noise-suppressor/rnnoise_simd.wasm?url"),
      ]);
      await ctx.audioWorklet.addModule(worklet.default);
      const binary = await pkg.loadRnnoise({
        url: wasm.default,
        simdUrl: simd.default,
      });
      node = new pkg.RnnoiseWorkletNode(ctx, { maxChannels: 1, wasmBinary: binary });
    } else if (mode === "gtcrn") {
      const [worklet, wasm] = await Promise.all([
        import("@sapphi-red/web-noise-suppressor/gtcrnWorklet.js?url"),
        import("@sapphi-red/web-noise-suppressor/gtcrn.wasm?url"),
      ]);
      await ctx.audioWorklet.addModule(worklet.default);
      const binary = await pkg.loadGtcrn({ url: wasm.default });
      node = new pkg.GtcrnWorkletNode(ctx, { maxChannels: 1, wasmBinary: binary });
    } else {
      const [worklet, wasm] = await Promise.all([
        import("@sapphi-red/web-noise-suppressor/speexWorklet.js?url"),
        import("@sapphi-red/web-noise-suppressor/speex.wasm?url"),
      ]);
      await ctx.audioWorklet.addModule(worklet.default);
      const binary = await pkg.loadSpeex({ url: wasm.default });
      node = new pkg.SpeexWorkletNode(ctx, { maxChannels: 1, wasmBinary: binary });
    }
    const dest = ctx.createMediaStreamDestination();
    configureMono(node, dest);
    node.connect(dest);
    return new SimpleWorkletEngine(mode, ctx, node, dest);
  }

  attach(input: MediaStream): void {
    void this.ctx.resume();
    this.input?.disconnect();
    this.source?.getTracks().forEach((t) => t.stop());
    this.source = input;
    this.input = this.ctx.createMediaStreamSource(input);
    this.input.connect(this.node);
  }

  get stream(): MediaStream {
    return this.dest.stream;
  }

  createBypass(input: MediaStream): MediaStream {
    const source = this.ctx.createMediaStreamSource(input);
    const dest = this.ctx.createMediaStreamDestination();
    configureMono(source, dest);
    source.connect(dest);
    return dest.stream;
  }

  close(): void {
    this.input?.disconnect();
    this.source?.getTracks().forEach((t) => t.stop());
    this.source = null;
    this.node.destroy?.();
    this.node.disconnect();
    void this.ctx.close();
  }
}

/** @workadventure/noise-suppression 的 DTLN / DeepFilterNet3 节点。 */
class WorkadventureEngine implements NoiseEngine {
  private input: MediaStreamAudioSourceNode | null = null;
  private source: MediaStream | null = null;

  private constructor(
    readonly mode: "dtln" | "deepfilternet3",
    private ctx: AudioContext,
    private handle: { node: AudioWorkletNode; dispose(): void },
    private dest: MediaStreamAudioDestinationNode,
  ) {}

  static async create(
    mode: "dtln" | "deepfilternet3",
    params: NoiseParams,
  ): Promise<WorkadventureEngine> {
    const sampleRate = mode === "dtln" ? 16000 : 48000;
    const ctx = new AudioContext({ sampleRate });
    await ctx.resume();
    if (Math.round(ctx.sampleRate) !== sampleRate) {
      void ctx.close();
      throw new Error(`当前设备不支持 ${sampleRate} Hz 音频，无法使用这个降噪模型`);
    }

    let handle: { node: AudioWorkletNode; dispose(): void };
    try {
      if (mode === "dtln") {
        // 运行时按模型选择加载，静态 import 会把约 6MB 模型打进主包。
        const mod = await import("@workadventure/noise-suppression/audio-worklet");
        const created = await mod.createNoiseSuppressionAudioWorklet(ctx, {
          bypassUntilReady: params.dtln.bypassUntilReady,
          readyTimeoutMs: params.dtln.readyTimeoutMs,
        });
        await created.ready;
        handle = created;
      } else {
        const mod = await import("@workadventure/noise-suppression/deepfilternet");
        const p = params.deepfilternet3;
        const created = await mod.createDeepFilterNetAudioWorklet(ctx, {
          speechAttenuationDb: p.speechAttenuationDb,
          pauseAttenuationDb: p.pauseAttenuationDb,
          pauseGateLookaheadFrames: p.pauseGateLookaheadFrames,
          minSpeechFrames: p.minSpeechFrames,
          postGain: p.postGain ? p.postGainOptions : false,
          maxLoad: p.maxLoad,
          bypassUntilReady: p.bypassUntilReady,
          readyTimeoutMs: p.readyTimeoutMs,
          onOverload: (load) => {
            setNoiseRuntimeStatus({
              requested: mode,
              actual: mode,
              state: "ready",
              message: `当前设备运行 DeepFilterNet3 较吃力（占用约 ${Math.round(load * 100)}%），建议换用 RNNoise 或 GTCRN`,
            });
          },
        });
        handle = created;
        await created.ready;
      }
    } catch (e) {
      void ctx.close();
      throw e;
    }

    const dest = ctx.createMediaStreamDestination();
    configureMono(handle.node, dest);
    handle.node.connect(dest);
    return new WorkadventureEngine(mode, ctx, handle, dest);
  }

  attach(input: MediaStream): void {
    void this.ctx.resume();
    this.input?.disconnect();
    this.source?.getTracks().forEach((t) => t.stop());
    this.source = input;
    this.input = this.ctx.createMediaStreamSource(input);
    this.input.connect(this.handle.node);
  }

  get stream(): MediaStream {
    return this.dest.stream;
  }

  createBypass(input: MediaStream): MediaStream {
    const source = this.ctx.createMediaStreamSource(input);
    const dest = this.ctx.createMediaStreamDestination();
    configureMono(source, dest);
    source.connect(dest);
    return dest.stream;
  }

  close(): void {
    this.input?.disconnect();
    this.source?.getTracks().forEach((t) => t.stop());
    this.source = null;
    this.handle.dispose();
    void this.ctx.close();
  }
}

/** 按模型 id 创建引擎；只有 WASM 模式可调用。 */
export async function createDenoiseEngine(
  mode: WasmNoiseMode,
  params: NoiseParams = getNoiseSettings().params,
): Promise<NoiseEngine> {
  switch (mode) {
    case "rnnoise":
    case "gtcrn":
    case "speex":
      return SimpleWorkletEngine.create(mode);
    case "dtln":
    case "deepfilternet3":
      return WorkadventureEngine.create(mode, params);
  }
}

export interface NoiseBenchmarkResult {
  mode: WasmNoiseMode;
  /** 从开始创建到模型/工作线程就绪的毫秒数。 */
  loadMs: number;
  sampleRate: number;
  trackState: MediaStreamTrackState;
}

export interface NoiseLatencyResult {
  mode: WasmNoiseMode;
  /** 开启降噪后，本机处理链路相对直通的额外延迟（毫秒）。 */
  addedLatencyMs: number;
  /** 互相关峰值，0–1；过低时结果不可信。 */
  confidence: number;
}

/**
 * 本地实测：不占用麦克风，用合成音频驱动当前模型并测量加载耗时。
 * 只验证模型能否初始化并持续产出 live track；不宣称等同于真实设备延迟。
 */
export async function benchmarkNoiseProvider(
  mode: WasmNoiseMode,
  params: NoiseParams = getNoiseSettings().params,
): Promise<NoiseBenchmarkResult> {
  const { promise: timeout, reject: rejectTimeout } =
    Promise.withResolvers<never>();
  const timer = setTimeout(
    () => rejectTimeout(new Error("模型启动超过 10 秒，请检查网络后重试")),
    10000,
  );
  try {
    return await Promise.race([measureNoiseProvider(mode, params), timeout]);
  } finally {
    clearTimeout(timer);
  }
}

async function measureNoiseProvider(
  mode: WasmNoiseMode,
  params: NoiseParams,
): Promise<NoiseBenchmarkResult> {
  const inputCtx = new AudioContext({ sampleRate: 48000 });
  const oscillator = inputCtx.createOscillator();
  const gain = inputCtx.createGain();
  const inputDest = inputCtx.createMediaStreamDestination();
  let engine: NoiseEngine | null = null;
  let outputCtx: AudioContext | null = null;
  try {
    await inputCtx.resume();
    oscillator.frequency.value = 440;
    gain.gain.value = 0.1;
    oscillator.connect(gain).connect(inputDest);
    oscillator.start();

    const started = performance.now();
    engine = await createDenoiseEngine(mode, params);
    const loadMs = Math.round(performance.now() - started);
    engine.attach(inputDest.stream);

    outputCtx = new AudioContext();
    await outputCtx.resume();
    const source = outputCtx.createMediaStreamSource(engine.stream);
    const analyser = outputCtx.createAnalyser();
    analyser.fftSize = 1024;
    source.connect(analyser);
    const { promise: settle, resolve: settled } = Promise.withResolvers<void>();
    setTimeout(settled, 800);
    await settle;
    const track = engine.stream.getAudioTracks()[0];
    return {
      mode,
      loadMs,
      sampleRate: track?.getSettings().sampleRate ?? 0,
      trackState: track?.readyState ?? "ended",
    };
  } finally {
    oscillator.stop();
    engine?.close();
    await outputCtx?.close().catch(() => undefined);
    await inputCtx.close().catch(() => undefined);
  }
}

/** 用同一段麦克风语音比较直通和降噪输出，互相关求额外延迟。 */
export async function measureNoiseLatency(
  mode: WasmNoiseMode,
  params: NoiseParams = getNoiseSettings().params,
  inputStream?: MediaStream,
): Promise<NoiseLatencyResult> {
  const { promise: timeout, reject: rejectTimeout } =
    Promise.withResolvers<never>();
  const timer = setTimeout(
    () => rejectTimeout(new Error("测量超时，请确认麦克风可用、网络正常后重试")),
    15000,
  );
  try {
    return await Promise.race([
      measureNoiseLatencyInner(mode, params, inputStream),
      timeout,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

function envelopeForCorrelation(samples: Float32Array, sampleRate: number): {
  values: Float32Array;
  stepSamples: number;
} {
  const stepSamples = Math.max(1, Math.round(sampleRate / 2000));
  const values = new Float32Array(Math.ceil(samples.length / stepSamples));
  for (let i = 0; i < values.length; i++) {
    let peak = 0;
    const end = Math.min(samples.length, (i + 1) * stepSamples);
    for (let j = i * stepSamples; j < end; j++) {
      peak = Math.max(peak, Math.abs(samples[j]));
    }
    values[i] = peak;
  }
  return { values, stepSamples };
}

function correlationAtLag(
  raw: Float32Array,
  processed: Float32Array,
  lag: number,
): number {
  let dot = 0;
  let rawEnergy = 0;
  let processedEnergy = 0;
  const start = Math.max(0, -lag);
  const end = Math.min(raw.length, processed.length - lag);
  for (let i = start; i < end; i++) {
    const a = raw[i];
    const b = processed[i + lag];
    dot += a * b;
    rawEnergy += a * a;
    processedEnergy += b * b;
  }
  if (rawEnergy <= 1e-9 || processedEnergy <= 1e-9) return 0;
  return dot / Math.sqrt(rawEnergy * processedEnergy);
}

async function measureNoiseLatencyInner(
  mode: WasmNoiseMode,
  params: NoiseParams,
  providedInput?: MediaStream,
): Promise<NoiseLatencyResult> {
  let input = providedInput ?? null;
  const ownsInput = !input;
  if (!input) {
    input = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: false,
        noiseSuppression: false,
        autoGainControl: false,
      },
    });
  }

  const inputCtx = new AudioContext({ sampleRate: 48000 });
  const captureCtx = new AudioContext({ sampleRate: 48000 });
  const engineInput = inputCtx.createMediaStreamDestination();
  let engine: NoiseEngine | null = null;
  let bypassStream: MediaStream | null = null;
  let processor: ScriptProcessorNode | null = null;
  const bypassChunks: Float32Array[] = [];
  const processedChunks: Float32Array[] = [];
  try {
    await inputCtx.resume();
    await captureCtx.resume();
    const micSource = inputCtx.createMediaStreamSource(input);
    micSource.connect(engineInput);

    engine = await createDenoiseEngine(mode, params);
    engine.attach(engineInput.stream);
    bypassStream = engine.createBypass(engineInput.stream);

    const bypassSource = captureCtx.createMediaStreamSource(bypassStream);
    const processedSource = captureCtx.createMediaStreamSource(engine.stream);
    const merger = captureCtx.createChannelMerger(2);
    bypassSource.connect(merger, 0, 0);
    processedSource.connect(merger, 0, 1);
    processor = captureCtx.createScriptProcessor(4096, 2, 2);
    const captureZero = captureCtx.createGain();
    captureZero.gain.value = 0;
    merger.connect(processor);
    processor.connect(captureZero);
    captureZero.connect(captureCtx.destination);
    processor.onaudioprocess = (event) => {
      bypassChunks.push(new Float32Array(event.inputBuffer.getChannelData(0)));
      processedChunks.push(new Float32Array(event.inputBuffer.getChannelData(1)));
    };

    const captured = Promise.withResolvers<void>();
    setTimeout(captured.resolve, 3500);
    await captured.promise;

    const bypass = new Float32Array(
      bypassChunks.reduce((sum, chunk) => sum + chunk.length, 0),
    );
    const processed = new Float32Array(
      processedChunks.reduce((sum, chunk) => sum + chunk.length, 0),
    );
    let offset = 0;
    for (let i = 0; i < bypassChunks.length; i++) {
      bypass.set(bypassChunks[i], offset);
      processed.set(processedChunks[i], offset);
      offset += bypassChunks[i].length;
    }
    const bypassEnvelope = envelopeForCorrelation(bypass, captureCtx.sampleRate);
    const processedEnvelope = envelopeForCorrelation(processed, captureCtx.sampleRate);
    const maxLag = Math.round((0.3 * captureCtx.sampleRate) / bypassEnvelope.stepSamples);
    let bestLag = 0;
    let bestScore = -1;
    for (let lag = 0; lag <= maxLag; lag++) {
      const score = correlationAtLag(
        bypassEnvelope.values,
        processedEnvelope.values,
        lag,
      );
      if (score > bestScore) {
        bestScore = score;
        bestLag = lag;
      }
    }
    if (bestScore < 0.35) {
      throw new Error("没有听清持续说话，请允许麦克风后连续说 3 秒再试");
    }
    return {
      mode,
      addedLatencyMs:
        (bestLag * bypassEnvelope.stepSamples * 1000) / captureCtx.sampleRate,
      confidence: bestScore,
    };
  } finally {
    if (processor) processor.onaudioprocess = null;
    engine?.close();
    bypassStream?.getTracks().forEach((track) => track.stop());
    engineInput.stream.getTracks().forEach((track) => track.stop());
    if (ownsInput) input?.getTracks().forEach((track) => track.stop());
    await captureCtx.close().catch(() => undefined);
    await inputCtx.close().catch(() => undefined);
  }
}
