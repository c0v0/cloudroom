import { useEffect, useState } from "react";
import {
  DEFAULT_DEEPFILTERNET3_PARAMS,
  DEFAULT_DTLN_PARAMS,
  DEFAULT_POST_GAIN_PARAMS,
  benchmarkNoiseProvider,
  isWasmNoiseMode,
  measureNoiseLatency,
  noiseErrorMessage,
  noiseProvider,
  type DeepFilterNetParams,
  type DtlnParams,
  type NoiseBenchmarkResult,
  type NoiseLatencyResult,
  type NoiseMode,
  type NoiseParams,
  type PostGainParams,
} from "../hooks/denoise";

interface NumberFieldProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  hint?: string;
  onCommit: (value: number) => void;
}

function NumberField({ label, value, min, max, step = 1, hint, onCommit }: NumberFieldProps) {
  const [draft, setDraft] = useState(String(value));

  useEffect(() => {
    setDraft(String(value));
  }, [value]);

  const commit = () => {
    const next = Number(draft);
    if (!Number.isFinite(next)) {
      setDraft(String(value));
      return;
    }
    const clamped = Math.min(max, Math.max(min, next));
    setDraft(String(clamped));
    if (clamped !== value) onCommit(clamped);
  };

  return (
    <label className="field noise-param">
      <span className="field-label">{label}</span>
      <span className="noise-param-control">
        <input
          type="number"
          min={min}
          max={max}
          step={step}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
          }}
        />
        {hint && <small className="hint">{hint}</small>}
      </span>
    </label>
  );
}

interface NoiseAdvancedProps {
  mode: NoiseMode;
  params: NoiseParams;
  onChange: (params: NoiseParams) => void;
}

/** 模型级高级参数；只在用户展开时显示，数值在失焦/回车后提交。 */
export function NoiseAdvanced({ mode, params, onChange }: NoiseAdvancedProps) {
  const [benchmark, setBenchmark] = useState<NoiseBenchmarkResult | null>(null);
  const [benchmarkBusy, setBenchmarkBusy] = useState(false);
  const [benchmarkError, setBenchmarkError] = useState<string | null>(null);
  const [latency, setLatency] = useState<NoiseLatencyResult | null>(null);
  const [latencyBusy, setLatencyBusy] = useState(false);
  const [latencyError, setLatencyError] = useState<string | null>(null);
  useEffect(() => {
    setBenchmark(null);
    setBenchmarkError(null);
    setLatency(null);
    setLatencyError(null);
  }, [mode, params]);
  if (mode === "off") return null;
  const info = noiseProvider(mode);
  const runBenchmark = async () => {
    if (!isWasmNoiseMode(mode)) return;
    setBenchmarkBusy(true);
    setBenchmarkError(null);
    try {
      setBenchmark(await benchmarkNoiseProvider(mode, params));
    } catch (error) {
      setBenchmark(null);
      setBenchmarkError(noiseErrorMessage(error));
    } finally {
      setBenchmarkBusy(false);
    }
  };
  const runLatency = async () => {
    if (!isWasmNoiseMode(mode)) return;
    setLatencyBusy(true);
    setLatencyError(null);
    try {
      setLatency(await measureNoiseLatency(mode, params));
    } catch (error) {
      setLatency(null);
      setLatencyError(noiseErrorMessage(error));
    } finally {
      setLatencyBusy(false);
    }
  };
  const updateDeepFilterNet = (patch: Partial<DeepFilterNetParams>) =>
    onChange({
      ...params,
      deepfilternet3: { ...params.deepfilternet3, ...patch },
    });
  const updateDtln = (patch: Partial<DtlnParams>) =>
    onChange({
      ...params,
      dtln: { ...params.dtln, ...patch },
    });
  const updatePostGain = (patch: Partial<PostGainParams>) =>
    updateDeepFilterNet({
      postGainOptions: {
        ...params.deepfilternet3.postGainOptions,
        ...patch,
      },
    });

  return (
    <details className="settings-details">
      <summary>高级设置</summary>
      <p className="hint">
        音频采样率：
        {info.sampleRate > 0 ? `${info.sampleRate} Hz` : "跟随系统"}
      </p>
      {isWasmNoiseMode(mode) && (
        <div className="noise-benchmark">
          <button
            type="button"
            className="link"
            disabled={benchmarkBusy}
            onClick={() => void runBenchmark()}
          >
            {benchmarkBusy ? "正在测试启动…" : "测试启动速度"}
          </button>
          {benchmark && (
            <span className="hint">
              启动约 {benchmark.loadMs}ms · {benchmark.sampleRate} Hz ·{" "}
              {benchmark.trackState === "live" ? "正常" : "未启动"}
            </span>
          )}
          {benchmarkError && <span className="error">启动测试失败：{benchmarkError}</span>}
          <span className="hint">只测启动速度，不代表实际通话延迟。</span>
        </div>
      )}
      {isWasmNoiseMode(mode) && (
        <div className="noise-benchmark">
          <button
            type="button"
            className="link"
            disabled={latencyBusy}
            onClick={() => void runLatency()}
          >
            {latencyBusy ? "正在测量，请连续说话…" : "测试降噪延迟"}
          </button>
          {latency && (
            <span className="hint">
              开启降噪后增加约 {latency.addedLatencyMs.toFixed(1)}ms
              （结果稳定度 {Math.round(latency.confidence * 100)}%）
            </span>
          )}
          {latencyError && <span className="error">延迟测试失败：{latencyError}</span>}
          <span className="hint">
            点击后允许麦克风，连续说 3 秒。会对比同一段声音开启/关闭降噪的延迟，
            只代表本机处理，不含网络和耳机、麦克风自身的延迟。
          </span>
        </div>
      )}

      {mode === "webrtc" && (
        <p className="hint">
          使用浏览器自带降噪，没有可调参数。回声消除和自动增益在上方设置。
        </p>
      )}

      {(mode === "rnnoise" || mode === "gtcrn" || mode === "speex") && (
        <p className="hint">这个模型没有可调参数，上方显示的是当前音频设置。</p>
      )}

      {mode === "dtln" && (
        <>
          <label className="check">
            <input
              type="checkbox"
              checked={params.dtln.bypassUntilReady}
              onChange={(e) => updateDtln({ bypassUntilReady: e.target.checked })}
            />
            <span>
              模型启动时先发原声
              <small>启动期间先发送原始声音，避免短暂静音。</small>
            </span>
          </label>
          <NumberField
            label="启动超时（毫秒）"
            value={params.dtln.readyTimeoutMs}
            min={1000}
            max={60000}
            step={1000}
            hint="超时后自动改用 WebRTC 降噪。"
            onCommit={(v) => updateDtln({ readyTimeoutMs: v })}
          />
          <p className="hint">DTLN 音质偏闷，会损失高频，适合低功耗设备。</p>
          <p className="hint">当前版本不提供线程和模型地址设置。</p>
          <button
            type="button"
            className="link"
            onClick={() =>
              onChange({ ...params, dtln: { ...DEFAULT_DTLN_PARAMS } })
            }
          >
            恢复 DTLN 默认设置
          </button>
        </>
      )}

      {mode === "deepfilternet3" && (
        <>
          <NumberField
            label="说话时背景声降低（dB）"
            value={params.deepfilternet3.speechAttenuationDb}
            min={0}
            max={100}
            hint="数值越大背景越安静，太大时字间可能像断音。"
            onCommit={(v) => updateDeepFilterNet({ speechAttenuationDb: v })}
          />
          <NumberField
            label="停顿时背景声降低（dB）"
            value={params.deepfilternet3.pauseAttenuationDb}
            min={0}
            max={100}
            hint="不高于“说话时”的数值时，关闭停顿降噪。"
            onCommit={(v) => updateDeepFilterNet({ pauseAttenuationDb: v })}
          />
          <NumberField
            label="停顿检测提前量（每格约 10 毫秒）"
            value={params.deepfilternet3.pauseGateLookaheadFrames}
            min={0}
            max={10}
            hint="数值越大越不容易削掉字头，但会增加延迟。"
            onCommit={(v) => updateDeepFilterNet({ pauseGateLookaheadFrames: v })}
          />
          <NumberField
            label="恢复背景声所需连续语音（帧）"
            value={params.deepfilternet3.minSpeechFrames}
            min={1}
            max={5}
            hint="2 或 3 可减少键盘声误触发。"
            onCommit={(v) => updateDeepFilterNet({ minSpeechFrames: v })}
          />
          <label className="check">
            <input
              type="checkbox"
              checked={params.deepfilternet3.postGain}
              onChange={(e) =>
                updateDeepFilterNet({ postGain: e.target.checked })
              }
            />
            <span>
              降噪后自动音量补偿
              <small>替代浏览器自动增益；开启时建议关闭上方自动增益。</small>
            </span>
          </label>
          {params.deepfilternet3.postGain && (
            <details className="settings-details">
              <summary>音量补偿细节</summary>
              <NumberField
                label="目标说话音量（dBFS）"
                value={params.deepfilternet3.postGainOptions.targetDb}
                min={-60}
                max={0}
                hint="补偿后希望达到的平均说话音量。"
                onCommit={(v) => updatePostGain({ targetDb: v })}
              />
              <NumberField
                label="最多提高音量（dB）"
                value={params.deepfilternet3.postGainOptions.maxGainDb}
                min={0}
                max={40}
                onCommit={(v) => updatePostGain({ maxGainDb: v })}
              />
              <NumberField
                label="最多降低音量（dB）"
                value={params.deepfilternet3.postGainOptions.minGainDb}
                min={-40}
                max={0}
                onCommit={(v) => updatePostGain({ minGainDb: v })}
              />
              <NumberField
                label="静音判断音量（dBFS）"
                value={params.deepfilternet3.postGainOptions.speechThresholdDb}
                min={-80}
                max={0}
                hint="低于这个音量不调整，避免把残留噪声放大。"
                onCommit={(v) => updatePostGain({ speechThresholdDb: v })}
              />
              <NumberField
                label="音量调节速度"
                value={params.deepfilternet3.postGainOptions.adaptDbPerFrame}
                min={0.001}
                max={1}
                step={0.01}
                onCommit={(v) => updatePostGain({ adaptDbPerFrame: v })}
              />
              <NumberField
                label="最大音量上限（0–1）"
                value={params.deepfilternet3.postGainOptions.ceiling}
                min={0.1}
                max={1}
                step={0.05}
                onCommit={(v) => updatePostGain({ ceiling: v })}
              />
              <button
                type="button"
                className="link"
                onClick={() =>
                  updateDeepFilterNet({
                    postGainOptions: { ...DEFAULT_POST_GAIN_PARAMS },
                  })
                }
              >
                恢复音量补偿默认值
              </button>
            </details>
          )}
          <NumberField
            label="性能保护阈值（0–1）"
            value={params.deepfilternet3.maxLoad}
            min={0}
            max={1}
            step={0.05}
            hint="0 关闭提示；设备跟不上时会提醒换轻量模型。"
            onCommit={(v) => updateDeepFilterNet({ maxLoad: v })}
          />
          <label className="check">
            <input
              type="checkbox"
              checked={params.deepfilternet3.bypassUntilReady}
              onChange={(e) =>
                updateDeepFilterNet({ bypassUntilReady: e.target.checked })
              }
            />
            <span>
              模型启动时先发原声
              <small>启动期间先发送原始声音，避免短暂静音。</small>
            </span>
          </label>
          <NumberField
            label="启动超时（毫秒）"
            value={params.deepfilternet3.readyTimeoutMs}
            min={1000}
            max={60000}
            step={1000}
            hint="超时后自动改用 WebRTC 降噪。"
            onCommit={(v) => updateDeepFilterNet({ readyTimeoutMs: v })}
          />
          <p className="hint">
            建议先点“测试降噪延迟”确认设备是否流畅；如果卡顿，换用更轻量的模型。
          </p>
          <button
            type="button"
            className="link"
            onClick={() =>
              onChange({
                ...params,
                deepfilternet3: { ...DEFAULT_DEEPFILTERNET3_PARAMS },
              })
            }
          >
            恢复 DeepFilterNet3 默认设置
          </button>
        </>
      )}

    </details>
  );
}
