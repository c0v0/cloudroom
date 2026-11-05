import { FormEvent, useState } from "react";
import { api } from "../api";

function defaultLocalTime(): string {
  const d = new Date(Date.now() + 5 * 60 * 1000);
  const local = new Date(d.getTime() - d.getTimezoneOffset() * 60000);
  return local.toISOString().slice(0, 16);
}

export function ScheduleDialog({
  code,
  body,
  onClose,
  onScheduled,
  onNotice,
  onError,
}: {
  code: string;
  body: string;
  onClose: () => void;
  onScheduled: () => void;
  onNotice: (msg: string) => void;
  onError: (msg: string) => void;
}) {
  const [sendAt, setSendAt] = useState(defaultLocalTime);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!sendAt || busy) return;
    setBusy(true);
    try {
      await api(`/api/v1/rooms/${code}/messages/schedule`, {
        method: "POST",
        body: JSON.stringify({
          body,
          send_at: new Date(sendAt).toISOString(),
        }),
      });
      onNotice("定时消息已创建。");
      onScheduled();
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
        aria-label="定时发送"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2>定时发送</h2>
          <button className="modal-close" aria-label="关闭定时发送" onClick={onClose}>
            ×
          </button>
        </div>
        <p className="hint">{body.slice(0, 160)}</p>
        <form onSubmit={submit}>
          <label>
            发送时间
            <input
              type="datetime-local"
              value={sendAt}
              min={defaultLocalTime()}
              onChange={(e) => setSendAt(e.target.value)}
            />
          </label>
          <button disabled={busy || !sendAt}>{busy ? "创建中…" : "创建定时消息"}</button>
        </form>
      </div>
    </div>
  );
}
