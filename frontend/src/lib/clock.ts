/** 以服务端 `hello.now` 校准本机时钟：在线状态判断不能依赖客户端时间。 */
let offsetMs = 0;

export function setServerNow(iso: string | undefined): void {
  if (!iso) return;
  const t = Date.parse(iso);
  if (Number.isFinite(t)) offsetMs = Date.now() - t;
}

export function serverNow(): number {
  return Date.now() - offsetMs;
}
