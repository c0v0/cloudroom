/**
 * Spike 003 页面：@jitsi/rnnoise-wasm 在 48kHz AudioWorklet 上处理 480 帧。
 * 管线与 frontend/src/hooks/denoise.ts 相同（worklet 收集 480 帧 -> 主线程
 * rnnoise 处理 -> 回传），额外记录处理耗时；并做一组 passthrough 对照，
 * 证明 worklet 通路本身能无损传声（rnnoise 对纯噪声输出接近静音是预期行为）。
 */
import { createRNNWasmModule } from "@jitsi/rnnoise-wasm";
import rnnoiseWasmUrl from "@jitsi/rnnoise-wasm/dist/rnnoise.wasm?url";

const FRAME = 480;

const WORKLET_CODE = `
let buf = new Float32Array(480);
let pos = 0;
let out = null;
let outReady = false;
class RnnWorklet extends AudioWorkletProcessor {
  constructor() {
    super();
    this.port.onmessage = (e) => {
      out = e.data;
      outReady = true;
    };
  }
  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (!input || !output) return true;
    for (let i = 0; i < input.length; i++) {
      buf[pos++] = input[i];
      if (pos === 480) {
        this.port.postMessage(buf);
        buf = new Float32Array(480);
        pos = 0;
      }
    }
    if (outReady && out) {
      output.set(out);
      out = null;
      outReady = false;
    } else {
      output.fill(0);
    }
    return true;
  }
}
class PassthroughWorklet extends AudioWorkletProcessor {
  process(inputs, outputs) {
    const input = inputs[0] && inputs[0][0];
    const output = outputs[0] && outputs[0][0];
    if (input && output) output.set(input);
    return true;
  }
}
registerProcessor("rnnoise-processor", RnnWorklet);
registerProcessor("passthrough-processor", PassthroughWorklet);
`;

class RnnProcessor {
  constructor(module) {
    this.module = module;
    this.statePtr = module._rnnoise_create();
    this.inPtr = module._malloc(FRAME * 4);
    this.outPtr = module._malloc(FRAME * 4);
  }
  static async create() {
    const module = await createRNNWasmModule({ locateFile: () => rnnoiseWasmUrl });
    await new Promise((resolve) => {
      const check = () => {
        if (typeof module._rnnoise_create === "function" && module.HEAPF32) resolve();
        else setTimeout(check, 5);
      };
      check();
    });
    return new RnnProcessor(module);
  }
  process(frame) {
    this.module.HEAPF32.set(frame, this.inPtr / 4);
    this.module._rnnoise_process_frame(this.statePtr, this.inPtr, this.outPtr);
    frame.set(this.module.HEAPF32.subarray(this.outPtr / 4, this.outPtr / 4 + FRAME));
    return frame;
  }
}

/** 确定性白噪声缓冲（0.2 幅度），不依赖假麦设备。 */
function noiseBuffer(ctx) {
  const buf = ctx.createBuffer(1, ctx.sampleRate * 2, ctx.sampleRate);
  const d = buf.getChannelData(0);
  let seed = 12345;
  for (let i = 0; i < d.length; i++) {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    d[i] = (seed / 0x3fffffff - 1) * 0.2;
  }
  return buf;
}

const rms = (analyser, scratch) => {
  analyser.getFloatTimeDomainData(scratch);
  let sum = 0;
  for (const v of scratch) sum += v * v;
  return Math.sqrt(sum / scratch.length);
};

window.__spike003 = {
  /** 离线紧循环：只测 rnnoise 每帧处理成本（排除音频线程调度与沙箱负载）。 */
  async offline({ frames = 300 } = {}) {
    const rnn = await RnnProcessor.create();
    const frame = new Float32Array(FRAME);
    let seed = 999;
    for (let i = 0; i < FRAME; i++) {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      frame[i] = (seed / 0x3fffffff - 1) * 0.2;
    }
    const durations = [];
    for (let i = 0; i < frames; i++) {
      const t0 = performance.now();
      rnn.process(frame);
      durations.push(performance.now() - t0);
    }
    const sorted = [...durations].sort((a, b) => a - b);
    const pick = (q) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * q))];
    return {
      ok: true,
      frames,
      p50ms: Math.round(pick(0.5) * 100) / 100,
      p95ms: Math.round(pick(0.95) * 100) / 100,
      maxMs: Math.round(sorted[sorted.length - 1] * 100) / 100,
    };
  },

  /** mode: "rnnoise" | "passthrough" */
  async run({ mode, seconds = 2 } = {}) {
    const errors = [];
    const ctx = new AudioContext({ sampleRate: 48000 });
    if (ctx.state !== "running") {
      try {
        await ctx.resume();
      } catch (err) {
        errors.push("resume: " + String(err));
      }
    }
    if (Math.round(ctx.sampleRate) !== 48000) {
      return { ok: false, stage: "sample-rate", sampleRate: ctx.sampleRate };
    }

    const blobUrl = URL.createObjectURL(new Blob([WORKLET_CODE], { type: "application/javascript" }));
    try {
      await ctx.audioWorklet.addModule(blobUrl);
    } finally {
      URL.revokeObjectURL(blobUrl);
    }

    const rnn = mode === "rnnoise" ? await RnnProcessor.create() : null;
    const worklet = new AudioWorkletNode(ctx, mode === "rnnoise" ? "rnnoise-processor" : "passthrough-processor");
    const durations = [];
    if (rnn) {
      worklet.port.onmessage = (e) => {
        const t0 = performance.now();
        try {
          worklet.port.postMessage(rnn.process(e.data));
        } catch (err) {
          errors.push(String(err));
        }
        durations.push(performance.now() - t0);
      };
    }

    const src = ctx.createBufferSource();
    src.buffer = noiseBuffer(ctx);
    src.loop = true;
    const inAnalyser = ctx.createAnalyser();
    inAnalyser.fftSize = 2048;
    const outAnalyser = ctx.createAnalyser();
    outAnalyser.fftSize = 2048;
    // 必须把图接到 destination 才会被持续拉取：0 增益避免真实出声。
    const sink = ctx.createGain();
    sink.gain.value = 0;
    src.connect(inAnalyser);
    src.connect(worklet);
    worklet.connect(outAnalyser);
    worklet.connect(sink);
    sink.connect(ctx.destination);
    src.start();

    const scratchIn = new Float32Array(inAnalyser.fftSize);
    const scratchOut = new Float32Array(outAnalyser.fftSize);
    const inSamples = [];
    const outSamples = [];
    const t0 = performance.now();
    while (performance.now() - t0 < seconds * 1000) {
      await new Promise((r) => setTimeout(r, 50));
      inSamples.push(rms(inAnalyser, scratchIn));
      outSamples.push(rms(outAnalyser, scratchOut));
    }
    const avg = (a) => (a.length ? a.reduce((x, y) => x + y, 0) / a.length : 0);
    const sorted = [...durations].sort((a, b) => a - b);
    const p95 = sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))] : null;

    src.stop();
    src.disconnect();
    worklet.disconnect();
    const sampleRate = ctx.sampleRate;
    await ctx.close();

    return {
      ok: true,
      mode,
      sampleRate,
      inRms: Math.round(avg(inSamples) * 100000) / 100000,
      outRms: Math.round(avg(outSamples) * 100000) / 100000,
      frames: durations.length,
      p95ms: p95 === null ? null : Math.round(p95 * 100) / 100,
      maxMs: sorted.length ? Math.round(sorted[sorted.length - 1] * 100) / 100 : null,
      errors,
    };
  },
};
