import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { wsUrl } from "../api";

export interface RoomSocketEvent {
  type: string;
  [k: string]: unknown;
}

export interface RoomSocket {
  connected: boolean;
  voiceMembers: string[];
  send: (payload: unknown) => boolean;
}

const BACKOFF_MS = [1000, 2000, 4000, 8000, 16000, 30000];

/** 服务端下发的语音成员列表：非字符串项直接丢弃。 */
function asVoiceMembers(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
}

/**
 * Control-plane WebSocket for a room: chat, member events, presence and the
 * P2P voice signaling relay. Reconnects with bounded backoff instead of a
 * full page reload; the media session (LiveKit or P2P) is owned separately.
 */
export function useRoomSocket(
  code: string,
  userId: string | undefined,
  onEvent: (e: RoomSocketEvent) => void,
): RoomSocket {
  const [connected, setConnected] = useState(false);
  const [voiceMembers, setVoiceMembers] = useState<string[]>([]);
  const socketRef = useRef<WebSocket | null>(null);
  const onEventRef = useRef(onEvent);
  onEventRef.current = onEvent;
  const closedRef = useRef(false);
  const attemptRef = useRef(0);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    closedRef.current = false;
    setVoiceMembers([]); // 切换房间时清空上一房间的语音在场状态
    let ws: WebSocket | null = null;

    const scheduleReconnect = () => {
      const delay = BACKOFF_MS[Math.min(attemptRef.current, BACKOFF_MS.length - 1)];
      attemptRef.current += 1;
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      timerRef.current = window.setTimeout(connect, delay);
    };

    const connect = () => {
      if (closedRef.current) return;
      try {
        ws = new WebSocket(wsUrl(code));
      } catch {
        scheduleReconnect();
        return;
      }
      socketRef.current = ws;
      ws.onopen = () => {
        attemptRef.current = 0;
        setConnected(true);
      };
      ws.onmessage = (ev) => {
        let e: RoomSocketEvent;
        try {
          e = JSON.parse(ev.data as string) as RoomSocketEvent;
        } catch {
          return;
        }
        if (e.type === "hello") {
          setVoiceMembers(asVoiceMembers(e.voice_members));
        } else if (e.type === "voice_state") {
          setVoiceMembers(asVoiceMembers(e.members));
        }
        onEventRef.current(e);
      };
      ws.onclose = () => {
        setConnected(false);
        if (closedRef.current) return;
        scheduleReconnect();
      };
    };

    connect();
    return () => {
      closedRef.current = true;
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
      ws?.close();
      socketRef.current = null;
    };
  }, [code, userId]);

  const send = useCallback((payload: unknown) => {
    const ws = socketRef.current;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(payload));
      return true;
    }
    return false;
  }, []);

  // 返回值必须稳定：调用方会把它放进 useEffect 依赖数组（语音静音状态同步），
  // 每次渲染新建对象会让该 effect 每帧重跑并无限 setState，主线程被占满。
  return useMemo(() => ({ connected, voiceMembers, send }), [connected, voiceMembers, send]);
}
