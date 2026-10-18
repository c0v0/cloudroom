/**
 * 交互频率限制：对用户主动触发的、会与服务器通信的操作做节流，
 * 防止疯狂点击导致广播/请求刷屏。两个模式：
 * - leading：窗口内第一次立即执行，其余丢弃（动作类：加入/离开语音、切换降噪）。
 * - trailing：窗口内合并为最后一次执行（状态类：静音状态广播，以最新状态为准）。
 */

export function throttleLeading<T extends unknown[]>(
  fn: (...args: T) => void,
  ms: number,
): (...args: T) => void {
  let last = 0;
  return (...args: T) => {
    const now = Date.now();
    if (now - last < ms) return;
    last = now;
    fn(...args);
  };
}

export function throttleTrailing<T extends unknown[]>(
  fn: (...args: T) => void,
  ms: number,
): (...args: T) => void {
  let timer: number | null = null;
  let pending = false;
  return (...args: T) => {
    pending = true;
    if (timer !== null) return;
    timer = window.setTimeout(() => {
      timer = null;
      if (pending) {
        pending = false;
        fn(...args);
      }
    }, ms);
  };
}
