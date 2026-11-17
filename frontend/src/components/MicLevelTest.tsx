import { useEffect, useRef, useState } from "react";
import { openMicrophone } from "../hooks/audioCapture";
import { getMicId } from "../hooks/devices";
import { getNoiseSettings } from "../hooks/denoise";

/** 设置页里的本地输入电平测试：只显示当前麦克风电平，不进入语音、不发送到服务器。 */
export function MicLevelTest({ gateDb }: { gateDb: number | null }) {
  const [testing, setTesting] = useState(false);
  const [levelDb, setLevelDb] = useState<number | null>(null);
  const [peakDb, setPeakDb] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const ctxRef = useRef<AudioContext | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const timerRef = useRef<number | null>(null);

  const stop = () => {
    if (timerRef.current !== null) window.clearInterval(timerRef.current);
    timerRef.current = null;
    sourceRef.current?.disconnect();
    sourceRef.current = null;
    void ctxRef.current?.close().catch(() => undefined);
    ctxRef.current = null;
    streamRef.current?.getTracks().forEach((t) => t.stop());
    streamRef.current = null;
    setTesting(false);
    setLevelDb(null);
    setPeakDb(null);
  };

  const start = async () => {
    setError(null);
    setTesting(true);
    try {
      const noise = getNoiseSettings();
      const capture = await openMicrophone({
        aec: noise.aec,
        mode: noise.mode,
        agc: noise.agc,
        deviceId: getMicId() || undefined,
      });
      streamRef.current = capture.stream;
      const ctx = new AudioContext();
      ctxRef.current = ctx;
      void ctx.resume();
      const source = ctx.createMediaStreamSource(capture.stream);
      sourceRef.current = source;
      const analyser = ctx.createAnalyser();
      analyser.fftSize = 1024;
      source.connect(analyser);
      const buf = new Float32Array(analyser.fftSize);
      timerRef.current = window.setInterval(() => {
        analyser.getFloatTimeDomainData(buf);
        let sum = 0;
        let peak = 0;
        for (let i = 0; i < buf.length; i++) {
          const v = buf[i];
          sum += v * v;
          peak = Math.max(peak, Math.abs(v));
        }
        const rms = Math.sqrt(sum / buf.length);
        setLevelDb(rms > 1e-7 ? 20 * Math.log10(rms) : -100);
        setPeakDb(peak > 1e-7 ? 20 * Math.log10(peak) : -100);
      }, 100);
    } catch (e) {
      stop();
      setError(e instanceof Error ? e.message : "无法打开麦克风。");
    }
  };

  useEffect(() => stop, []);

  const pct = levelDb === null ? 0 : Math.max(0, Math.min(100, ((levelDb + 60) / 40) * 100));
  const thresholdPct =
    gateDb === null ? null : Math.max(0, Math.min(100, ((gateDb + 60) / 40) * 100));

  return (
    <div className="mic-level-test">
      <div className="mic-level-head">
        <button type="button" className="link" onClick={() => (testing ? stop() : void start())}>
          {testing ? "停止音量测试" : "测试当前音量"}
        </button>
        {levelDb !== null && <b>RMS {levelDb.toFixed(1)} dBFS</b>}
        {peakDb !== null && <b>峰值 {peakDb.toFixed(1)} dBFS</b>}
      </div>
      {testing && levelDb !== null && (
        <>
          <div className="mic-level-meter" aria-label="当前麦克风电平">
            <span className="mic-level-fill" style={{ width: `${pct}%` }} />
            {thresholdPct !== null && (
              <i
                className="mic-level-threshold"
                style={{ left: `${thresholdPct}%` }}
                title={`发送门限 ${gateDb} dBFS`}
              />
            )}
          </div>
          <small className="mic-level-status">
            {gateDb === null
              ? `当前 ${levelDb.toFixed(0)} dBFS；发送门限未开启`
              : levelDb >= gateDb
                ? `当前高于门限 ${gateDb} dBFS，会发送`
                : `当前低于门限 ${gateDb} dBFS，不发送`}
          </small>
        </>
      )}
      {testing && peakDb !== null && peakDb > -3 && (
        <small className="mic-level-warning">
          输入峰值接近过载，建议降低系统麦克风音量或关闭自动增益。
        </small>
      )}
      {testing && levelDb === null && !error && <small className="mic-level-status">正在检测…</small>}
      {error && <small className="mic-level-error">{error}</small>}
    </div>
  );
}
