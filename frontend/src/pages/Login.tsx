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
  const [challenge, setChallenge] = useState<string | null>(null);
  const [totpCode, setTotpCode] = useState("");
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
      if (challenge) {
        await api("/api/v1/auth/login/totp", {
          method: "POST",
          body: JSON.stringify({ challenge_token: challenge, code: totpCode }),
        });
        await refresh();
        navigate("/");
        return;
      }
      const r = await api<{ totp_required?: boolean; challenge_token?: string }>(
        "/api/v1/auth/login",
        { method: "POST", body: JSON.stringify({ username, password }) },
      );
      if (r.totp_required && r.challenge_token) {
        setChallenge(r.challenge_token);
        set密码("");
        setTotpCode("");
        return;
      }
      await refresh();
      navigate("/");
    } catch (err) {
      const status = errorStatus(err);
      if (status === 401 || status === 403) {
        set密码("");
        setTotpCode("");
        if (!challenge) passwordRef.current?.focus();
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
        {challenge ? (
          <>
            <p className="hint">请输入验证器中的 6 位动态码，或一条恢复码。</p>
            <label htmlFor="login-totp">动态码</label>
            <input
              id="login-totp"
              value={totpCode}
              onChange={(e) => setTotpCode(e.target.value)}
              autoFocus
              inputMode="numeric"
              autoComplete="one-time-code"
            />
            <button type="submit" disabled={busy || !totpCode.trim()}>
              {busy ? "验证中…" : "验证"}
            </button>
            <button
              type="button"
              className="link"
              onClick={() => {
                setChallenge(null);
                setTotpCode("");
                setError(null);
              }}
            >
              返回重新输入密码
            </button>
          </>
        ) : (
          <>
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
          </>
        )}
      </form>
    </div>
  );
}
