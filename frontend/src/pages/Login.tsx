import { FormEvent, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { api, apiOrigin } from "../api";
import { useAuth } from "../auth";

function errorStatus(err: unknown): number | undefined {
  if (typeof err !== "object" || err === null || !("status" in err)) return undefined;
  return typeof err.status === "number" ? err.status : undefined;
}

export default function Login() {
  const [username, set用户名] = useState("");
  const [password, set密码] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const passwordRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();
  const { refresh } = useAuth();
  const [params] = useSearchParams();
  const [sso, setSso] = useState<{ enabled: boolean; label: string } | null>(null);

  useEffect(() => {
    if (params.get("sso_error")) setError("SSO 登录失败，请联系管理员。");
    api<{ oidc?: { enabled: boolean; label: string } }>("/api/v1/config/public")
      .then((r) => setSso(r.oidc ?? null))
      .catch(() => undefined);
    // Only the initial error query matters.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api("/api/v1/auth/login", { method: "POST", body: JSON.stringify({ username, password }) });
      await refresh();
      navigate("/");
    } catch (err) {
      const status = errorStatus(err);
      if (status === 401 || status === 403) {
        set密码("");
        passwordRef.current?.focus();
      }
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
          ref={passwordRef}
          type="password"
          value={password}
          onChange={(e) => set密码(e.target.value)}
          autoComplete="current-password"
        />
        <button type="submit" disabled={busy}>{busy ? "登录中…" : "登录"}</button>
        {sso?.enabled && (
          <p className="hint">
            <a className="link" href={`${apiOrigin()}/api/v1/auth/oidc/login?next=${encodeURIComponent("/")}`}>
              {sso.label}
            </a>
          </p>
        )}
        <p className="hint">
          没有账号？<Link to="/register">注册</Link>（需要邀请）
        </p>
        <p className="hint">忘记密码请联系管理员重置。</p>
      </form>
    </div>
  );
}
