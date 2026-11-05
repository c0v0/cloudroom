import { FormEvent, useEffect, useState } from "react";
import { api, assetUrl } from "../api";

interface Emoji {
  id: string;
  name: string;
  url: string;
}

export function EmojisTab() {
  const [items, setItems] = useState<Emoji[]>([]);
  const [name, setName] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    try {
      const r = await api<{ emojis: Emoji[] }>("/api/v1/emojis");
      setItems(r.emojis);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const upload = async (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim() || !file || busy) return;
    setBusy(true);
    try {
      const form = new FormData();
      form.append("name", name.trim());
      form.append("file", file);
      await api("/api/v1/admin/emojis", { method: "POST", body: form });
      setName("");
      setFile(null);
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    if (!window.confirm("删除该 emoji？历史消息中的 :name: 将显示为文本。")) return;
    try {
      await api(`/api/v1/admin/emojis/${id}`, { method: "DELETE" });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className="card">
      <h2>自定义 Emoji</h2>
      <p className="hint">名称 2–32 位小写字母/数字/_-+；图片 ≤256 KiB、≤128×128。</p>
      {error && <p className="error">{error}</p>}
      <form className="row-actions" onSubmit={upload}>
        <input
          value={name}
          placeholder="例如 party"
          maxLength={32}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          type="file"
          accept="image/png,image/jpeg,image/webp,image/gif"
          onChange={(e) => setFile(e.target.files?.[0] ?? null)}
        />
        <button disabled={busy || !name.trim() || !file}>
          {busy ? "上传中…" : "上传"}
        </button>
      </form>
      <ul className="room-list">
        {items.map((e) => (
          <li key={e.id}>
            <img
              className="custom-emoji"
              src={assetUrl(e.url) ?? e.url}
              alt={`:${e.name}:`}
            />
            <code>:{e.name}:</code>
            <button className="link danger" onClick={() => void remove(e.id)}>
              删除
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
