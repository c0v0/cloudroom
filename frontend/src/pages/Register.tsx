import { FormEvent, useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";

export default function Register() {
  const [params] = useSearchParams();
  const hashToken = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  // Capture once: the raw token must not stay in the URL, and the live
  // query params are cleared below before the form is ever submitted.
  const [inviteCode, setInviteCode] = useState(
    () => params.get("bootstrap") || params.get("invite") || hashToken.get("bootstrap") || hashToken.get("invite") || "",
  );
  const [username, set用户名] = useState("");
  const [password, set密码] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [openRegistration, setOpenRegistration] = useState(false);
  const navigate = useNavigate();
  const { refresh } = useAuth();

  // 公开注册模式下不要求邀请码；邀请制仍强制。
  useEffect(() => {
    void Promise.resolve(api<{ registration_mode: string }>("/api/v1/config/public"))
      .then((c) => setOpenRegistration(c?.registration_mode === "open"))
      .catch(() => {});
  }, []);

  // The raw invite token must never stay in the address bar after success.
  useEffect(() => {
    if (params.get("bootstrap") || params.get("invite") || hashToken.get("bootstrap") || hashToken.get("invite")) {
      history.replaceState(null, "", "/register");
    }
  }, [params]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    if (password !== confirm) {
      setError("两次输入的密码不一致");
      setBusy(false);
      return;
    }
    const token = inviteCode.trim();
    if (!token && !openRegistration) {
      setError("请输入邀请码（注册需要管理员邀请或引导链接）");
      setBusy(false);
      return;
    }
    try {
      await api("/api/v1/auth/register", {
        method: "POST",
        body: JSON.stringify({ username, password, token: token || undefined }),
      });
      await refresh();
      navigate("/");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const fromLink = !!params.get("bootstrap") || !!params.get("invite") || !!hashToken.get("bootstrap") || !!hashToken.get("invite");

  return (
    <div className="page center">
      <form className="card" onSubmit={submit}>
        <h1>注册</h1>
        <p className="hint">
          {fromLink
            ? "注册链接已自动填入邀请码，可直接提交。"
            : "注册需要管理员邀请码。"}
        </p>
        {error && <p className="error">{error}</p>}
        <label htmlFor="reg-username">用户名</label>
        <input
          id="reg-username"
          value={username}
          onChange={(e) => set用户名(e.target.value)}
          autoFocus
          autoComplete="username"
        />
        <label htmlFor="reg-invite">邀请码</label>
        <input
          id="reg-invite"
          value={inviteCode}
          onChange={(e) => setInviteCode(e.target.value)}
          placeholder="管理员发放的注册邀请码"
          autoComplete="off"
        />
        <label htmlFor="reg-password">密码</label>
        <input
          id="reg-password"
          type="password"
          value={password}
          onChange={(e) => set密码(e.target.value)}
          autoComplete="new-password"
        />
        <label htmlFor="reg-confirm">确认密码</label>
        <input
          id="reg-confirm"
          type="password"
          value={confirm}
          onChange={(e) => setConfirm(e.target.value)}
          autoComplete="new-password"
        />
        <button disabled={busy}>{busy ? "注册中…" : "注册"}</button>
      </form>
    </div>
  );
}
