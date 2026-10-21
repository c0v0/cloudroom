import { useEffect, useState } from "react";
import { Invite, api } from "../api";

export interface CreatedInvite {
  url: string;
  token: string;
}

export interface InvitesViewProps {
  invites: Invite[];
  created: CreatedInvite | null;
  copied: string | null;
  error: string | null;
  onCreate: () => void;
  onRevoke: (inviteId: string) => void;
  onCopyLink: () => void;
  onCopyToken: () => void;
}

export function InvitesView({
  invites,
  created,
  copied,
  error,
  onCreate,
  onRevoke,
  onCopyLink,
  onCopyToken,
}: InvitesViewProps) {
  return (
    <section>
      <button onClick={onCreate}>生成注册邀请</button>
      {created && (
        <p className="ok">
          注册链接（仅显示一次）：<code>{created.url}</code>
          <span className="row-actions">
            <button onClick={onCopyLink}>复制链接</button>
            <button onClick={onCopyToken}>复制邀请码</button>
            {copied && <span className="tag ok">已复制{copied}</span>}
          </span>
        </p>
      )}
      {error && <p className="error">{error}</p>}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>创建时间</th>
            <th>过期时间</th>
            <th>使用者</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {invites.map((i) => (
            <tr key={i.invite_id}>
              <td>{new Date(i.created_at).toLocaleString()}</td>
              <td>{new Date(i.expires_at).toLocaleString()}</td>
              <td>{i.consumed_by ? i.consumed_by.slice(0, 8) : "—"}</td>
              <td>
                <span className={`tag ${i.active ? "ok" : "warn"}`}>
                  {i.consumed_at ? "已使用" : i.revoked_at ? "已撤销" : i.active ? "正常" : "已过期"}
                </span>
              </td>
              <td>
                {i.active && (
                  <button onClick={() => onRevoke(i.invite_id)}>撤销</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
    </section>
  );
}

export function InvitesTab() {
  const [invites, setInvites] = useState<Invite[]>([]);
  const [created, setCreated] = useState<CreatedInvite | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    api<{ invites: Invite[] }>("/api/v1/admin/invites")
      .then((r) => setInvites(r.invites))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);

  const create = async () => {
    try {
      const r = await api<{ token: string; register_url: string }>("/api/v1/admin/invites", {
        method: "POST",
        body: "{}",
      });
      setCreated({ url: location.origin + r.register_url, token: r.token });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const revoke = async (id: string) => {
    try {
      await api(`/api/v1/admin/invites/${id}/revoke`, { method: "POST", body: "{}" });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const copy = (text: string, label: string) => {
    void navigator.clipboard.writeText(text).then(
      () => setCopied(label),
      () => setCopied(null)
    );
  };

  return (
    <InvitesView
      invites={invites}
      created={created}
      copied={copied}
      error={error}
      onCreate={() => void create()}
      onRevoke={(id) => void revoke(id)}
      onCopyLink={() => created && copy(created.url, "链接")}
      onCopyToken={() => created && copy(created.token, "邀请码")}
    />
  );
}
