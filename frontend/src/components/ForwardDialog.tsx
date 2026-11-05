import { FormEvent, useEffect, useState } from "react";
import type { Message, Room } from "../api";
import { api } from "../api";

export function ForwardDialog({
  code,
  message,
  onClose,
  onNotice,
  onError,
}: {
  code: string;
  message: Message;
  onClose: () => void;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
}) {
  const [rooms, setRooms] = useState<Room[]>([]);
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api<{ rooms: Room[] }>("/api/v1/me/rooms")
      .then((r) =>
        setRooms(
          r.rooms.filter((x) => x.state === "active" && x.code !== code),
        ),
      )
      .catch((e) => onError(e instanceof Error ? e.message : String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!target || busy) return;
    setBusy(true);
    try {
      await api(`/api/v1/rooms/${code}/messages/${message.id}/forward`, {
        method: "POST",
        body: JSON.stringify({ target_room_code: target }),
      });
      onNotice(`已转发到 /${target}`);
      onClose();
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="转发消息"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2>转发消息</h2>
          <button className="modal-close" aria-label="关闭转发" onClick={onClose}>
            ×
          </button>
        </div>
        <p className="hint">{message.body.slice(0, 160)}</p>
        <form onSubmit={submit}>
          <label>
            目标房间
            <select value={target} onChange={(e) => setTarget(e.target.value)}>
              <option value="">请选择</option>
              {rooms.map((r) => (
                <option key={r.code} value={r.code}>
                  /{r.code}（{r.kind === "direct" ? "私聊" : "房间"}）
                </option>
              ))}
            </select>
          </label>
          <button disabled={!target || busy}>{busy ? "转发中…" : "转发"}</button>
        </form>
      </div>
    </div>
  );
}
