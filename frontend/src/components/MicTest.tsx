import { useMicTest } from "../hooks/useMicTest";

/** 个人设置里的本地麦克风测试：录制、原声/降噪对比，不进入房间语音。 */
export function MicTest() {
  const micTest = useMicTest();
  return (
    <section className="card">
      <h3>麦克风测试</h3>
      <p className="hint">录制一段声音，对比原声和降噪效果。</p>
      {micTest.error && <p className="error">{micTest.error}</p>}
      {micTest.state === "idle" && (
        <button onClick={micTest.startRecording}>开始录制</button>
      )}
      {micTest.state === "recording" && (
        <div className="row-actions">
          <span className="rec-dot" />
          <span>录制中 {micTest.seconds}s / 60s…</span>
          <button onClick={micTest.stopRecording}>停止录制</button>
        </div>
      )}
      {micTest.state === "recorded" && (
        <div className="row-actions">
          <span>已录制 {micTest.seconds}s</span>
          <button
            className={micTest.playMode === "raw" ? "active" : ""}
            onClick={() => micTest.play("raw")}
          >
            播放原声
          </button>
          <button
            className={micTest.playMode === "processed" ? "active" : ""}
            disabled={!micTest.recordedMode || micTest.recordedMode === "off"}
            title={
              micTest.recordedMode === "off"
                ? "当前未启用降噪，请先在上方选择降噪模式"
                : micTest.recordedMode === "rnnoise"
                  ? "rnnoise 降噪效果"
                  : "WebRTC 降噪效果"
            }
            onClick={() => micTest.play("processed")}
          >
            播放降噪效果（{micTest.recordedMode === "rnnoise" ? "rnnoise" : micTest.recordedMode === "webrtc" ? "WebRTC" : "未启用"}）
          </button>
          <button onClick={micTest.startRecording}>重新录制</button>
        </div>
      )}
      {micTest.state === "playing" && (
        <button onClick={micTest.stopPlay}>停止播放</button>
      )}
    </section>
  );
}
