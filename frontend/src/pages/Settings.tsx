import { FormEvent, useEffect, useRef, useState } from "react";
import { api, invalidateMe, Role } from "../api";
import { useAuth } from "../auth";
import { getNoiseSettings, setNoisePrefs } from "../hooks/denoise";
import type { NoiseMode } from "../hooks/denoise";
import { getDynamicsSettings, setDynamicsPrefs } from "../hooks/remoteAudio";
import { getVadThreshold, setVadThreshold } from "../hooks/volumeMeter";
import { getThemeMode, setThemeMode } from "../hooks/theme";
import type { ThemeMode } from "../hooks/theme";

export function roleLabel(role: Role): string {
  switch (role) {
    case "super_admin":
      return "超级管理员";
    case "admin":
      return "管理员";
    case "user":
      return "用户";
    case "guest":
      return "访客";
  }
}

const AVATAR_SIZE = 256;

/** 图片 cover 到 256×256 的基础缩放（图片自身缩放，不含用户缩放）。 */
function coverScale(img: HTMLImageElement): number {
  return Math.max(AVATAR_SIZE / img.naturalWidth, AVATAR_SIZE / img.naturalHeight);
}

/** 拖动偏移钳制：保证图片始终覆盖裁剪框。 */
function clampOffset(o: { x: number; y: number }, img: HTMLImageElement, scale: number) {
  const sc = coverScale(img) * scale;
  const maxX = Math.max(0, (img.naturalWidth * sc - AVATAR_SIZE) / 2);
  const maxY = Math.max(0, (img.naturalHeight * sc - AVATAR_SIZE) / 2);
  return {
    x: Math.min(maxX, Math.max(-maxX, o.x)),
    y: Math.min(maxY, Math.max(-maxY, o.y)),
  };
}

/** 头像手动裁剪：拖动图片调整位置 + 滑块缩放，方形裁剪框，确认后生成 256×256。 */
function AvatarCrop({
  file,
  onCancel,
  onDone,
}: {
  file: File;
  onCancel: () => void;
  onDone: (blob: Blob) => void;
}) {
  const [img, setImg] = useState<HTMLImageElement | null>(null);
  const [scale, setScale] = useState(1);
  const [offset, setOffset] = useState({ x: 0, y: 0 });
  const [error, setError] = useState<string | null>(null);
  const dragRef = useRef<{ px: number; py: number; ox: number; oy: number } | null>(null);

  useEffect(() => {
    const url = URL.createObjectURL(file);
    const i = new Image();
    i.onload = () => {
      setImg(i);
      setScale(1);
      setOffset({ x: 0, y: 0 });
    };
    i.onerror = () => setError("图片无法解码");
    i.src = url;
    return () => URL.revokeObjectURL(url);
  }, [file]);

  const onPointerDown = (e: React.PointerEvent) => {
    dragRef.current = { px: e.clientX, py: e.clientY, ox: offset.x, oy: offset.y };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    const d = dragRef.current;
    if (!d || !img) return;
    setOffset(
      clampOffset({ x: d.ox + (e.clientX - d.px), y: d.oy + (e.clientY - d.py) }, img, scale),
    );
  };
  const onPointerUp = () => {
    dragRef.current = null;
  };

  const confirm = () => {
    if (!img) return;
    const canvas = document.createElement("canvas");
    canvas.width = AVATAR_SIZE;
    canvas.height = AVATAR_SIZE;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    // 由显示状态反算源图裁剪区域。
    const sc = coverScale(img) * scale;
    const sx = img.naturalWidth / 2 - (AVATAR_SIZE / 2 - offset.x) / sc;
    const sw = AVATAR_SIZE / sc;
    const sy = img.naturalHeight / 2 - (AVATAR_SIZE / 2 - offset.y) / sc;
    const sh = AVATAR_SIZE / sc;
    ctx.drawImage(img, sx, sy, sw, sh, 0, 0, AVATAR_SIZE, AVATAR_SIZE);
    canvas.toBlob((b) => {
      if (b) onDone(b);
    }, "image/png");
  };

  return (
    <div className="avatar-crop">
      {error && <p className="error">{error}</p>}
      {img && (
        <>
          <div
            className="crop-box"
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
          >
            <img
              src={img.src}
              alt="裁剪预览"
              draggable={false}
              style={{
                width: `${AVATAR_SIZE * coverScale(img) * scale}px`,
                height: `${AVATAR_SIZE * coverScale(img) * scale}px`,
                transform: `translate(-50%, -50%) translate(${offset.x}px, ${offset.y}px)`,
              }}
            />
          </div>
          <label className="field">
            <span className="field-label">缩放</span>
            <input
              type="range"
              min={1}
              max={3}
              step={0.05}
              value={scale}
              onChange={(e) => {
                const s = Number(e.target.value);
                setScale(s);
                setOffset((o) => clampOffset(o, img, s));
              }}
            />
          </label>
          <div className="row-actions">
            <button onClick={onCancel}>取消</button>
            <button onClick={confirm}>确认裁剪</button>
          </div>
        </>
      )}
    </div>
  );
}

/**
 * 个人设置模态框（非独立页面）：声音设置（降噪/回声/增益，仅对用户自己生效）、
 * 头像、修改密码。声音设置写 localStorage 并广播订阅，语音会话即时重建。
 */
export default function SettingsModal({ onClose }: { onClose: () => void }) {
  const { me, refresh } = useAuth();
  const [noise, setNoise] = useState(() => getNoiseSettings());
  const [dynamics, setDynamics] = useState(() => getDynamicsSettings());
  const [dynAdvanced, setDynAdvanced] = useState(false);
  const [vad, setVad] = useState(() => Math.round(((0.2 - getVadThreshold()) / 0.19) * 100));
  const [theme, setTheme] = useState<ThemeMode>(() => getThemeMode());
  const [notify, setNotify] = useState(() => {
    try {
      return localStorage.getItem("cloudroom.notify") === "1";
    } catch {
      return false;
    }
  });
  const [sound, setSound] = useState(() => {
    try {
      return localStorage.getItem("cloudroom.sound") === "1";
    } catch {
      return false;
    }
  });
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [pendingFile, setPendingFile] = useState<File | null>(null);

  if (!me) return null;

  const changeNoise = (prefs: Partial<typeof noise>) => {
    const next = { ...noise, ...prefs };
    setNoise(next);
    setNoisePrefs(prefs);
  };

  const changePassword = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setOk(null);
    if (newPassword !== confirm) {
      setError("两次输入的新密码不一致");
      setBusy(false);
      return;
    }
    try {
      await api("/api/v1/me/password", {
        method: "POST",
        body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
      });
      setCurrentPassword("");
      setNewPassword("");
      setConfirm("");
      setOk("密码已修改。");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const uploadAvatar = async (blob: Blob) => {
    setError(null);
    try {
      // 上传用户裁剪后的 256×256 头像（后端仍保留 2MiB/1024 限制兜底）。
      const form = new FormData();
      form.append("avatar", blob, "avatar.png");
      await api("/api/v1/me/avatar", { method: "POST", body: form });
      // me() 有模块缓存：必须先失效再 refresh 才能拿到新头像。
      invalidateMe();
      await refresh();
      setOk("头像已更新。");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const removeAvatar = async () => {
    setError(null);
    try {
      await api("/api/v1/me/avatar", { method: "DELETE" });
      invalidateMe();
      await refresh();
      setOk("头像已删除。");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h2>个人设置</h2>
          <button className="modal-close" onClick={onClose} aria-label="关闭">
            ×
          </button>
        </div>
        {error && <p className="error">{error}</p>}
        {ok && <p className="ok">{ok}</p>}

        {me.user.role !== "guest" && (
          <div className="card">
            <h3>账号</h3>
            {pendingFile ? (
              <AvatarCrop
                file={pendingFile}
                onCancel={() => setPendingFile(null)}
                onDone={(blob) => {
                  setPendingFile(null);
                  void uploadAvatar(blob);
                }}
              />
            ) : (
              <div className="profile-row">
                {me.user.avatar_url ? (
                  <img className="avatar" src={me.user.avatar_url} alt="头像" />
                ) : (
                  <div className="avatar placeholder">
                    {(me.user.username || "?")[0].toUpperCase()}
                  </div>
                )}
                <div className="profile-meta">
                  <p className="profile-name">
                    <b>{me.user.username}</b>
                    <span className="tag">{roleLabel(me.user.role)}</span>
                  </p>
                  <div className="row-actions">
                    <label className="file-btn">
                      上传头像
                      <input
                        type="file"
                        accept="image/png,image/jpeg,image/webp"
                        onChange={(e) => {
                          const f = e.target.files?.[0];
                          if (f) setPendingFile(f);
                          e.target.value = "";
                        }}
                      />
                    </label>
                    {me.user.avatar_url && (
                      <button onClick={() => void removeAvatar()}>删除头像</button>
                    )}
                  </div>
                </div>
              </div>
            )}
          </div>
        )}

        <section className="card">
          <h3>声音</h3>
          <div className="field">
            <span className="field-label">降噪</span>
            <select
              value={noise.mode}
              onChange={(e) => changeNoise({ mode: e.target.value as NoiseMode })}
            >
              <option value="off">关</option>
              <option value="webrtc">WebRTC（浏览器内置）</option>
              <option value="rnnoise">rnnoise（本地 AI）</option>
            </select>
          </div>
          <label className="check">
            <input
              type="checkbox"
              checked={noise.aec}
              onChange={(e) => changeNoise({ aec: e.target.checked })}
            />
            <span>
              回声消除
              <small>外放开会时保持开启；戴耳机或误消第三方声音时可关</small>
            </span>
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={noise.agc}
              onChange={(e) => changeNoise({ agc: e.target.checked })}
            />
            <span>
              自动增益
              <small>说话距离变化自动补偿音量；环境噪声放大或音乐场景可关</small>
            </span>
          </label>
          <div className="dyn-block">
            <label className="check">
              <input
                type="checkbox"
                checked={dynamics.enabled}
                onChange={(e) => {
                  const next = { ...dynamics, enabled: e.target.checked };
                  setDynamics(next);
                  setDynamicsPrefs({ enabled: next.enabled });
                }}
              />
              <span>
                音量均衡（压缩 + 峰值限制）
                <small>压低大声、抬升小声，防止爆音；适合有人忽大忽小的场景</small>
              </span>
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={notify}
                onChange={(e) => {
                  const on = e.target.checked;
                  setNotify(on);
                  try {
                    localStorage.setItem("cloudroom.notify", on ? "1" : "0");
                  } catch {
                    /* 忽略 */
                  }
                  if (on && typeof Notification !== "undefined" && Notification.permission === "default") {
                    void Notification.requestPermission();
                  }
                }}
              />
              <span>
                桌面通知
                <small>房间不在当前页面时，新消息和被 @ 提醒弹出系统通知</small>
              </span>
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={sound}
                onChange={(e) => {
                  const on = e.target.checked;
                  setSound(on);
                  try {
                    localStorage.setItem("cloudroom.sound", on ? "1" : "0");
                  } catch {
                    /* 忽略 */
                  }
                }}
              />
              <span>
                新消息提示音
                <small>收到新消息时播放短提示音</small>
              </span>
            </label>
            <div className="field">
              <span className="field-label">主题</span>
              <select
                value={theme}
                onChange={(e) => {
                  const t = e.target.value as ThemeMode;
                  setTheme(t);
                  setThemeMode(t);
                }}
              >
                <option value="system">跟随系统</option>
                <option value="dark">深色</option>
                <option value="light">浅色</option>
              </select>
            </div>
            <label className="field">
              <span className="field-label">语音激活灵敏度（{vad}）</span>
              <input
                type="range"
                min={0}
                max={100}
                step={1}
                value={vad}
                onChange={(e) => {
                  const pct = Number(e.target.value);
                  setVad(pct);
                  // 越大越灵敏（阈值越小）。
                  setVadThreshold(0.2 - (pct / 100) * 0.19);
                }}
              />
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={dynamics.dtxOff}
                onChange={(e) => {
                  const next = { ...dynamics, dtxOff: e.target.checked };
                  setDynamics(next);
                  setDynamicsPrefs({ dtxOff: next.dtxOff });
                }}
              />
              <span>
                关闭静音压缩
                <small>音乐/持续环境声场景防误判静音；占用带宽更高</small>
              </span>
            </label>
            <button className="link" onClick={() => setDynAdvanced(!dynAdvanced)}>
              {dynAdvanced ? "收起高级参数" : "高级参数"}
            </button>
            {dynAdvanced && (
              <div className="dyn-params">
                <label className="field">
                  <span className="field-label">阈值（{dynamics.threshold} dB）</span>
                  <input
                    type="range"
                    min={-48}
                    max={0}
                    step={1}
                    value={dynamics.threshold}
                    onChange={(e) => {
                      const v = Number(e.target.value);
                      setDynamics((d) => ({ ...d, threshold: v }));
                      setDynamicsPrefs({ threshold: v });
                    }}
                  />
                </label>
                <label className="field">
                  <span className="field-label">比率（{dynamics.ratio}:1）</span>
                  <input
                    type="range"
                    min={1}
                    max={20}
                    step={1}
                    value={dynamics.ratio}
                    onChange={(e) => {
                      const v = Number(e.target.value);
                      setDynamics((d) => ({ ...d, ratio: v }));
                      setDynamicsPrefs({ ratio: v });
                    }}
                  />
                </label>
              </div>
            )}
          </div>
        </section>

        {me.user.role !== "guest" && (
          <form className="card" onSubmit={changePassword}>
            <h3>修改密码</h3>
            <input
              className="field"
              type="password"
              placeholder="当前密码"
              value={currentPassword}
              onChange={(e) => setCurrentPassword(e.target.value)}
            />
            <input
              className="field"
              type="password"
              placeholder="新密码"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
            />
            <input
              className="field"
              type="password"
              placeholder="确认新密码"
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
            <button disabled={busy || !currentPassword || !newPassword || !confirm}>
              {busy ? "提交中…" : "修改密码"}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
