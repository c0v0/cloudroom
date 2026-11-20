import { FormEvent, useState } from "react";
import { api } from "../api";
import { useAuth } from "../auth";

/**
 * Shown immediately after an administrator password reset. The server clears
 * the flag when the password is successfully replaced.
 */
export default function ForcePasswordChange() {
  const { refresh, logout } = useAuth();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (next !== confirm) {
      setError("两次输入的新密码不一致");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api("/api/v1/me/password", {
        method: "POST",
        body: JSON.stringify({ current_password: current, new_password: next }),
      });
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page center">
      <form className="card" onSubmit={submit}>
        <h1>需要修改密码</h1>
        <p className="hint">
          管理员已重置你的密码。请先设置新密码，再继续使用 CloudRoom。
        </p>
        {error && <p className="error">{error}</p>}
        <label htmlFor="force-current">临时密码</label>
        <input
          id="force-current"
          type="password"
          autoComplete="current-password"
          value={current}
          onChange={(e) => setCurrent(e.target.value)}
          autoFocus
        />
        <label htmlFor="force-new">新密码</label>
        <input
          id="force-new"
          type="password"
          autoComplete="new-password"
          value={next}
          onChange={(e) => setNext(e.target.value)}
        />
        <label htmlFor="force-confirm">确认新密码</label>
        <input
          id="force-confirm"
          type="password"
          autoComplete="new-password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        <button type="submit" disabled={busy || !current || !next || !confirm}>
          {busy ? "保存中…" : "保存新密码"}
        </button>
        <button type="button" className="link" onClick={() => void logout()}>
          退出登录
        </button>
      </form>
    </div>
  );
}
