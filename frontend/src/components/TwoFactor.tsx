import { FormEvent, useState } from "react";
import { api } from "../api";

export function TwoFactor({
  enabled,
  onChanged,
}: {
  enabled: boolean;
  onChanged: () => Promise<void> | void;
}) {
  const [stage, setStage] = useState<"idle" | "setup" | "recovery" | "disable">("idle");
  const [secret, setSecret] = useState("");
  const [otpauth, setOtpauth] = useState("");
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [recovery, setRecovery] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const start = async () => {
    setBusy(true);
    setError(null);
    setOk(null);
    try {
      const r = await api<{ secret: string; otpauth_url: string }>(
        "/api/v1/me/totp/setup",
        { method: "POST", body: "{}" },
      );
      setSecret(r.secret);
      setOtpauth(r.otpauth_url);
      setStage("setup");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const enable = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const r = await api<{ recovery_codes: string[] }>("/api/v1/me/totp/enable", {
        method: "POST",
        body: JSON.stringify({ code, password: password || undefined }),
      });
      setRecovery(r.recovery_codes);
      setStage("recovery");
      setCode("");
      setOk("两步验证已启用。请保存恢复码。");
      await onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const disable = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api("/api/v1/me/totp/disable", {
        method: "POST",
        body: JSON.stringify({ password: password || undefined, code }),
      });
      setStage("idle");
      setPassword("");
      setCode("");
      setOk("两步验证已关闭。");
      await onChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card">
      <h3>两步验证</h3>
      {error && <p className="error">{error}</p>}
      {ok && <p className="ok">{ok}</p>}
      {enabled ? (
        <>
          <p className="hint">已启用。登录时需要输入动态码或恢复码。</p>
          {stage !== "disable" ? (
            <button onClick={() => setStage("disable")}>关闭两步验证</button>
          ) : (
            <form onSubmit={disable}>
              <input
                className="field"
                type="password"
                placeholder="当前密码（OIDC 账号可留空）"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <input
                className="field"
                inputMode="numeric"
                placeholder="6 位动态码或恢复码"
                value={code}
                onChange={(e) => setCode(e.target.value)}
              />
              <div className="row-actions">
                <button disabled={busy || !code}>{busy ? "关闭中…" : "确认关闭"}</button>
                <button type="button" className="link" onClick={() => setStage("idle")}>
                  取消
                </button>
              </div>
            </form>
          )}
        </>
      ) : stage === "idle" ? (
        <>
          <p className="hint">使用 TOTP 应用（如 1Password、Aegis、Google Authenticator）。</p>
          <button onClick={() => void start()} disabled={busy}>
            {busy ? "准备中…" : "启用两步验证"}
          </button>
        </>
      ) : stage === "setup" ? (
        <form onSubmit={enable}>
          <p className="hint">在验证器应用中添加以下密钥：</p>
          <code className="invite-url">{secret}</code>
          <p className="hint">
            或使用链接：<a href={otpauth}>{otpauth}</a>
          </p>
          <input
            className="field"
            type="password"
            placeholder="当前密码（OIDC 账号可留空）"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <input
            className="field"
            inputMode="text"
            placeholder="输入 6 位动态码或恢复码"
            value={code}
            onChange={(e) => setCode(e.target.value)}
          />
          <div className="row-actions">
            <button disabled={busy || code.length < 6}>{busy ? "确认中…" : "确认启用"}</button>
            <button type="button" className="link" onClick={() => setStage("idle")}>
              取消
            </button>
          </div>
        </form>
      ) : (
        <>
          <p className="hint">恢复码只显示一次，请离线保存：</p>
          <ul className="room-list">
            {recovery.map((c) => (
              <li key={c}>
                <code>{c}</code>
              </li>
            ))}
          </ul>
          <button onClick={() => setStage("idle")}>我已保存</button>
        </>
      )}
    </section>
  );
}
