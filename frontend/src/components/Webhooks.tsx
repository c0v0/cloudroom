import { FormEvent, useEffect, useState } from "react";
import { api } from "../api";

interface Webhook {
  id: string;
  name: string;
  created_at: string;
  revoked_at: string | null;
  last_used_at: string | null;
}

interface CreatedWebhook {
  url: string;
  token: string;
}

export function Webhooks({
  code,
  onError,
  onNotice,
}: {
  code: string;
  onError: (msg: string) => void;
  onNotice: (msg: string) => void;
}) {
  const [hooks, setHooks] = useState<Webhook[]>([]);
  const [name, setName] = useState("");
  const [created, setCreated] = useState<CreatedWebhook | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const r = await api<{ webhooks: Webhook[] }>(`/api/v1/rooms/${code}/webhooks`);
      setHooks(r.webhooks);
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code]);

  const create = async (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim() || busy) return;
    setBusy(true);
    try {
      const r = await api<{ url: string; token: string }>(
        `/api/v1/rooms/${code}/webhooks`,
        { method: "POST", body: JSON.stringify({ name }) },
      );
      setCreated({ url: r.url, token: r.token });
      setName("");
      onNotice("Webhook 已创建，令牌只显示一次。");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const revoke = async (id: string) => {
    if (!window.confirm("撤销后该 Webhook 立即失效，继续？")) return;
    try {
      await api(`/api/v1/rooms/${code}/webhooks/${id}/revoke`, {
        method: "POST",
        body: "{}",
      });
      onNotice("Webhook 已撤销。");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <section className="card">
      <h2>Webhook</h2>
      <p className="hint">
        外部系统可向 <code>POST /api/v1/webhooks/&lt;token&gt;</code> 发送
        <code>{'{"body":"..."}'}</code>，消息以房主身份进入本房间。
      </p>
      <form className="row-actions" onSubmit={create}>
        <input
          value={name}
          maxLength={64}
          placeholder="名称（例如 CI）"
          onChange={(e) => setName(e.target.value)}
        />
        <button disabled={busy || !name.trim()}>{busy ? "创建中…" : "创建"}</button>
      </form>
      {created && (
        <div className="invite-url">
          <code>{created.url}</code>
          <button
            onClick={() => {
              void navigator.clipboard.writeText(created.url);
              onNotice("已复制 Webhook URL。");
            }}
          >
            复制
          </button>
          <p className="hint">令牌只显示一次；丢失后请撤销并重新创建。</p>
        </div>
      )}
      <ul className="room-list">
        {hooks.map((h) => (
          <li key={h.id}>
            <span className="code">{h.name}</span>
            <span className={`tag ${h.revoked_at ? "warn" : "ok"}`}>
              {h.revoked_at ? "已撤销" : "有效"}
            </span>
            <span className="muted">
              {h.last_used_at
                ? `最近使用 ${new Date(h.last_used_at).toLocaleString()}`
                : "尚未使用"}
            </span>
            {!h.revoked_at && (
              <button className="link danger" onClick={() => void revoke(h.id)}>
                撤销
              </button>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
