/**
 * 入会/离会提示音：Web Audio 合成（上行双音=加入，下行=离开），
 * 音量克制（0.15），无需任何音频资源文件。
 */
let ctx: AudioContext | null = null;

export function playChime(kind: "join" | "leave"): void {
  try {
    ctx ??= new AudioContext();
    if (ctx.state === "suspended") void ctx.resume();
    const t = ctx.currentTime;
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    const base = kind === "join" ? 660 : 520;
    osc.frequency.setValueAtTime(base, t);
    osc.frequency.linearRampToValueAtTime(
      base * (kind === "join" ? 1.35 : 0.75),
      t + 0.09,
    );
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.15, t + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.3);
    osc.connect(gain).connect(ctx.destination);
    osc.start(t);
    osc.stop(t + 0.32);
  } catch {
    /* 提示音失败不影响功能 */
  }
}

/** 新消息提示音：短促双音（区别于入会/离会音）。 */
export function playMessageChime(): void {
  try {
    ctx ??= new AudioContext();
    if (ctx.state === "suspended") void ctx.resume();
    const t = ctx.currentTime;
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.type = "sine";
    osc.frequency.setValueAtTime(880, t);
    osc.frequency.setValueAtTime(1320, t + 0.08);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.08, t + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.2);
    osc.connect(gain).connect(ctx.destination);
    osc.start(t);
    osc.stop(t + 0.24);
  } catch {
    /* 提示音失败不影响功能 */
  }
}
