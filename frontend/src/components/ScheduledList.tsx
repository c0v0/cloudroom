import { useEffect, useState } from "react";
import { api } from "../api";

interface Scheduled {
  id: string;
  body: string;
  send_at: string;
  thread_root_id: string | null;
}

export function ScheduledList({
  code,
  onError,
  onNotice,
}: {
  code: string;
  onError: (msg: string) => void;
  onNotice: (msg: string) => void;
}) {
  const [items, setItems] = useState<Scheduled[]>([]);

  const load = async () => {
    try {
      const r = await api<{ scheduled: Scheduled[] }>(
        `/api/v1/rooms/${code}/scheduled`,
      );
      setItems(r.scheduled);
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  const cancel = async (id: string) => {
    try {
      await api(`/api/v1/rooms/${code}/scheduled/${id}/cancel`, {
        method: "POST",
        body: "{}",
      });
      onNotice("定时消息已取消。");
      await load();
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    }
  };

  if (items.length === 0) return null;
  return (
    <section className="card">
      <h2>定时消息</h2>
      <ul className="room-list">
        {items.map((m) => (
          <li key={m.id}>
            <span className="muted">{new Date(m.send_at).toLocaleString()}</span>
            <span className="search-snippet">{m.body}</span>
            <button className="link danger" onClick={() => void cancel(m.id)}>
              取消
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
