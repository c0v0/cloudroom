/**
 * 麦克风测试（纯本地，零服务器资源）：录制双轨——原始轨（无任何处理）
 * 与当前降噪模型（WebRTC / RNNoise / GTCRN / DTLN / DeepFilterNet3 / Speex）的实时处理轨，播放时"原声 vs 降噪效果"
 * A/B 对比。同一段录音听差异，比实时监听更能分辨降噪效果。
 * 60 秒上限自动停止。off 模式无处理轨（无对比）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { createDenoiseEngine, getNoiseSettings, isWasmNoiseMode } from "./denoise";
import type { NoiseEngine, NoiseMode } from "./denoise";
import { ensureAutoGainControl } from "./audioCapture";

type MicTestState = "idle" | "recording" | "recorded" | "playing";
type PlayMode = "raw" | "processed";

interface MicTestHandle {
  state: MicTestState;
  /** 当前播放的对比轨（playing 时有效）。 */
  playMode: PlayMode | null;
  /** 本次录制时的降噪模式（off 时无对比轨）。 */
  recordedMode: NoiseMode | null;
  seconds: number;
  error: string | null;
  startRecording: () => void;
  stopRecording: () => void;
  play: (mode: PlayMode) => void;
  stopPlay: () => void;
  discard: () => void;
}

const MAX_SECONDS = 60;

export function useMicTest(): MicTestHandle {
  const [state, setState] = useState<MicTestState>("idle");
  const [playMode, setPlayMode] = useState<PlayMode | null>(null);
  const [recordedMode, setRecordedMode] = useState<NoiseMode | null>(null);
  const [seconds, setSeconds] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const recsRef = useRef<MediaRecorder[]>([]);
  const streamsRef = useRef<MediaStream[]>([]);
  const engineRef = useRef<NoiseEngine | null>(null);
  const blobsRef = useRef<{ raw: Blob | null; processed: Blob | null }>({ raw: null, processed: null });
  const tickRef = useRef<number | null>(null);
  const maxTimerRef = useRef<number | null>(null);
  const playCtxRef = useRef<AudioContext | null>(null);
  const playSrcRef = useRef<AudioBufferSourceNode | null>(null);

  const stopPlay = useCallback(() => {
    playSrcRef.current?.stop();
    playSrcRef.current = null;
    void playCtxRef.current?.close();
    playCtxRef.current = null;
    setPlayMode(null);
    setState((s) => (s === "playing" ? "recorded" : s));
  }, []);

  /** 通用播放：解码 blob 并播放。 */
  const playBlob = useCallback(
    async (blob: Blob, mode: PlayMode) => {
      const ctx = new AudioContext();
      playCtxRef.current = ctx;
      const buf = await ctx.decodeAudioData(await blob.arrayBuffer());
      const node = ctx.createBufferSource();
      node.buffer = buf;
      // A/B 对比时统一响度：raw/webrtc 轨没有 rnnoise 的增益补偿，
      // 直接播放会明显偏小；按 RMS 归一化到同一目标电平，避免误判降噪效果。
      let sum = 0;
      let count = 0;
      for (let ch = 0; ch < buf.numberOfChannels; ch++) {
        const data = buf.getChannelData(ch);
        for (let i = 0; i < data.length; i++) {
          sum += data[i] * data[i];
          count++;
        }
      }
      const rms = count > 0 ? Math.sqrt(sum / count) : 0;
      const gain = ctx.createGain();
      gain.gain.value = rms > 0.001 ? Math.min(4, Math.max(0.25, 0.1 / rms)) : 1;
      node.connect(gain);
      gain.connect(ctx.destination);
      playSrcRef.current = node;
      node.onended = () => stopPlay();
      setPlayMode(mode);
      setState("playing");
      node.start();
    },
    [stopPlay],
  );

  const stopRecording = useCallback(() => {
    if (tickRef.current !== null) {
      window.clearInterval(tickRef.current);
      tickRef.current = null;
    }
    if (maxTimerRef.current !== null) {
      window.clearTimeout(maxTimerRef.current);
      maxTimerRef.current = null;
    }
    recsRef.current.forEach((r) => {
      if (r.state === "recording") r.stop();
    });
  }, []);

  const startRecording = useCallback(() => {
    setError(null);
    stopPlay();
    const settings = getNoiseSettings();
    const mode = settings.mode;
    void (async () => {
      let raw: MediaStream;
      try {
        // 原始轨：关闭全部处理，保证对比基准干净。
        raw = await navigator.mediaDevices.getUserMedia({
          audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false },
        });
      } catch (e) {
        const rawErr = e instanceof Error ? e.message : String(e);
        if (/NotFoundError|Requested device not found|not found/i.test(rawErr)) {
          setError("未找到麦克风设备，请检查设备连接或浏览器权限。");
        } else if (/NotAllowedError|PermissionDenied|permission/i.test(rawErr)) {
          setError("麦克风权限被拒绝，请在浏览器地址栏允许使用麦克风。");
        } else {
          setError(rawErr);
        }
        return;
      }
      // 处理轨：按当前降噪模式实时处理（webrtc 用浏览器约束，rnnoise 过 WASM 管线）。
      let processed: MediaStream | null = null;
      if (mode === "webrtc") {
        try {
          processed = await navigator.mediaDevices.getUserMedia({
            audio: {
              echoCancellation: false,
              noiseSuppression: true,
              autoGainControl: settings.agc,
            },
          });
          await ensureAutoGainControl(processed, settings.agc, {
            echoCancellation: false,
            noiseSuppression: true,
          });
        } catch {
          processed = null;
        }
      } else if (isWasmNoiseMode(mode)) {
        try {
          engineRef.current?.close();
          const engine = await createDenoiseEngine(mode);
          engine.attach(raw);
          engineRef.current = engine;
          processed = engine.stream;
        } catch {
          processed = null;
        }
      }
      const mime = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : "";
      const mkRec = (stream: MediaStream): Promise<Blob> =>
        new Promise((resolve) => {
          const rec = new MediaRecorder(stream, mime ? { mimeType: mime } : undefined);
          const chunks: BlobPart[] = [];
          rec.ondataavailable = (e) => {
            if (e.data.size > 0) chunks.push(e.data);
          };
          rec.onstop = () => {
            resolve(new Blob(chunks, { type: rec.mimeType || "audio/webm" }));
          };
          rec.start();
          recsRef.current.push(rec);
        });
      streamsRef.current = [raw, processed].filter((s): s is MediaStream => !!s);
      recsRef.current = [];
      blobsRef.current = { raw: null, processed: null };
      setRecordedMode(mode);
      setSeconds(0);
      setState("recording");
      tickRef.current = window.setInterval(() => {
        setSeconds((s) => {
          if (s + 1 >= MAX_SECONDS) stopRecording();
          return s + 1;
        });
      }, 1000);
      maxTimerRef.current = window.setTimeout(() => stopRecording(), MAX_SECONDS * 1000);
      const [rawBlob, processedBlob] = await Promise.all([mkRec(raw), processed ? mkRec(processed) : Promise.resolve(null)]);
      blobsRef.current = { raw: rawBlob, processed: processedBlob };
      streamsRef.current.forEach((s) => s.getTracks().forEach((t) => t.stop()));
      streamsRef.current = [];
      setState("recorded");
    })();
  }, [stopPlay, stopRecording]);

  const play = useCallback(
    (mode: PlayMode) => {
      const blob = mode === "raw" ? blobsRef.current.raw : blobsRef.current.processed;
      if (!blob) return;
      setError(null);
      void playBlob(blob, mode).catch((e) => {
        setError(e instanceof Error ? e.message : String(e));
        setPlayMode(null);
        setState("recorded");
      });
    },
    [playBlob],
  );

  const discard = useCallback(() => {
    stopPlay();
    stopRecording();
    engineRef.current?.close();
    engineRef.current = null;
    blobsRef.current = { raw: null, processed: null };
    setSeconds(0);
    setState("idle");
  }, [stopPlay, stopRecording]);

  // 卸载时清理全部资源。
  useEffect(() => {
    return () => {
      if (tickRef.current !== null) window.clearInterval(tickRef.current);
      if (maxTimerRef.current !== null) window.clearTimeout(maxTimerRef.current);
      recsRef.current.forEach((r) => {
        if (r.state === "recording") r.stop();
      });
      streamsRef.current.forEach((s) => s.getTracks().forEach((t) => t.stop()));
      engineRef.current?.close();
      playSrcRef.current?.stop();
      void playCtxRef.current?.close();
    };
  }, []);

  return { state, playMode, recordedMode, seconds, error, startRecording, stopRecording, play, stopPlay, discard };
}
