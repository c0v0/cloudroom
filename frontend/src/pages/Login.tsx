import { FormEvent, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";

export default function Login() {
  const [username, set用户名] = useState("");
  const [password, set密码] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const navigate = useNavigate();
  const { refresh } = useAuth();

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api("/api/v1/auth/login", { method: "POST", body: JSON.stringify({ username, password }) });
      await refresh();
      navigate("/");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page center">
      <form className="card" onSubmit={submit}>
        <h1>登录</h1>
        {error && <p className="error">{error}</p>}
        <label htmlFor="login-username">用户名</label>
        <input
          id="login-username"
          value={username}
          onChange={(e) => set用户名(e.target.value)}
          autoFocus
          autoComplete="username"
        />
        <label htmlFor="login-password">密码</label>
        <input
          id="login-password"
          type="password"
          value={password}
          onChange={(e) => set密码(e.target.value)}
          autoComplete="current-password"
        />
        <button type="submit" disabled={busy}>{busy ? "登录中…" : "登录"}</button>
        <p className="hint">
          没有账号？<Link to="/register">注册</Link>（需要邀请）
        </p>
        <p className="hint">忘记密码请联系管理员重置。</p>
      </form>
    </div>
  );
}
