import { FormEvent, useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { api, apiOrigin, BookmarkEntry, BookmarkTag, Room, RoomRequest } from "../api";
import { useAuth } from "../auth";
import { normalizeRoomCode } from "../lib/roomCode";
import { loadBitrateTiers } from "../lib/voiceOptions";
import { BookmarkExport, bookmarkFilename, bookmarksToCsv, bookmarksToMarkdown } from "../lib/bookmarkExport";

interface InboxRoom { room_code: string; kind: string; unread: number; mentions: number; thread_unread: number; }
interface InboxMention { message_id: string; room_code: string; body: string; created_at: string; sender_nickname: string | null; unread: boolean; thread_root_id: string | null; }
interface Inbox { total_unread: number; total_mentions: number; rooms: InboxRoom[]; mentions: InboxMention[]; }

interface SearchHit {
  message_id: string;
  room_code: string;
  body: string;
  created_at: string;
  sender_nickname?: string | null;
}

export default function Home() {
  const { me } = useAuth();
  const [rooms, setRooms] = useState<Room[]>([]);
  const [requests, setRequests] = useState<RoomRequest[]>([]);
  const [inbox, setInbox] = useState<Inbox | null>(null);
  const [bookmarks, setBookmarks] = useState<BookmarkEntry[]>([]);
  const [bookmarkQ, setBookmarkQ] = useState("");
  const [bookmarkSelected, setBookmarkSelected] = useState<Set<string>>(new Set());
  const [bookmarkBusy, setBookmarkBusy] = useState(false);
  const [exportFormat, setExportFormat] = useState<"json" | "csv" | "md">("json");
  const [backup, setBackup] = useState<{ enabled: boolean; last_backup_at: string | null; available: boolean } | null>(null);
  const [backupVersions, setBackupVersions] = useState<{ id: string; created_at: string; size: number }[]>([]);
  const [backupVersion, setBackupVersion] = useState("");
  const [bookmarkTags, setBookmarkTags] = useState<BookmarkTag[]>([]);
  const [tagFilter, setTagFilter] = useState("");
  const [tagEditId, setTagEditId] = useState<string | null>(null);
  const [tagDraft, setTagDraft] = useState("");
  const [manageTag, setManageTag] = useState("");
  const [tagRename, setTagRename] = useState("");
  const [batchTag, setBatchTag] = useState("");
  const [searchQ, setSearchQ] = useState("");
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searchBusy, setSearchBusy] = useState(false);
  const [searchDone, setSearchDone] = useState(false);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [note, setNote] = useState("");
  const [desiredCode, setDesiredCode] = useState("");
  const [mediaMode, setMediaMode] = useState<"p2p" | "sfu">("p2p");
  const [audioBitrate, setAudioBitrate] = useState(48);
  const [wantAttachments, setWantAttachments] = useState(false);
  const [bitrateTiers, setBitrateTiers] = useState<
    { kbps: number; min_owner_role: string }[]
  >([]);
  const [error, setError] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      const r = await api<{ rooms: Room[] }>("/api/v1/me/rooms");
      setRooms(r.rooms);
      const q = await api<{ requests: RoomRequest[] }>("/api/v1/room-requests");
      setRequests(q.requests);
      const i = await api<Inbox>("/api/v1/inbox");
      setInbox(i);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const loadBookmarkTags = async () => {
    try {
      const r = await api<{ tags: BookmarkTag[] }>("/api/v1/me/bookmarks/tags");
      setBookmarkTags(r.tags);
    } catch {
      setBookmarkTags([]);
    }
  };

  const fetchBookmarks = async (query: string) => {
    try {
      const suffix = query.trim()
        ? `&q=${encodeURIComponent(query.trim())}`
        : "";
      const tag = tagFilter ? `&tag=${encodeURIComponent(tagFilter)}` : "";
      const b = await api<{ bookmarks: BookmarkEntry[] }>(
        `/api/v1/me/bookmarks?limit=50${suffix}${tag}`,
      );
      setBookmarks(b.bookmarks);
      setBookmarkSelected(new Set());
      await loadBookmarkTags();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  useEffect(() => {
    const timer = window.setTimeout(
      () => void fetchBookmarks(bookmarkQ),
      bookmarkQ.trim() ? 250 : 0,
    );
    return () => window.clearTimeout(timer);
    // fetchBookmarks is intentionally recreated; only filters drive reloads.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bookmarkQ, tagFilter]);

  const exportBookmarks = async () => {
    setBookmarkBusy(true);
    try {
      const params = new URLSearchParams();
      if (bookmarkQ.trim()) params.set("q", bookmarkQ.trim());
      if (tagFilter) params.set("tag", tagFilter);
      const suffix = params.toString();
      const data = await api<BookmarkExport>(
        `/api/v1/me/bookmarks/export${suffix ? `?${suffix}` : ""}`,
      );
      const content =
        exportFormat === "csv"
          ? bookmarksToCsv(data.bookmarks)
          : exportFormat === "md"
            ? bookmarksToMarkdown(data.bookmarks, data.exported_at)
            : JSON.stringify(data, null, 2);
      const type =
        exportFormat === "csv"
          ? "text/csv;charset=utf-8"
          : exportFormat === "md"
            ? "text/markdown;charset=utf-8"
            : "application/json;charset=utf-8";
      const blob = new Blob([content], { type });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = bookmarkFilename(exportFormat);
      document.body.append(a);
      a.click();
      a.remove();
      URL.revokeObjectURL(url);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  useEffect(() => {
    void api<{ enabled: boolean; last_backup_at: string | null; available: boolean }>(
      "/api/v1/me/bookmarks/backup",
    )
      .then(setBackup)
      .catch(() => undefined);
  }, []);

  const loadBackupVersions = async () => {
    try {
      const r = await api<{ versions: { id: string; created_at: string; size: number }[] }>(
        "/api/v1/me/bookmarks/backup/versions",
      );
      setBackupVersions(r.versions);
      setBackupVersion((current) =>
        r.versions.some((v) => v.id === current) ? current : r.versions[0]?.id || "",
      );
    } catch {
      setBackupVersions([]);
    }
  };

  useEffect(() => {
    if (backup?.available) void loadBackupVersions();
    else setBackupVersions([]);
  }, [backup?.available]);

  const restoreBackup = async () => {
    if (!backupVersion) return;
    setBookmarkBusy(true);
    setOk(null);
    try {
      const r = await api<{ imported: number; skipped: number; invalid: number }>(
        "/api/v1/me/bookmarks/backup/restore",
        { method: "POST", body: JSON.stringify({ version_id: backupVersion }) },
      );
      setOk(
        `恢复完成：新增 ${r.imported} 条，已存在 ${r.skipped} 条，无效 ${r.invalid} 条。`,
      );
      await fetchBookmarks(bookmarkQ);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  const toggleBackup = async (enabled: boolean) => {
    try {
      const r = await api<{ enabled: boolean; last_backup_at: string | null }>(
        "/api/v1/me/bookmarks/backup",
        { method: "PUT", body: JSON.stringify({ enabled }) },
      );
      setBackup({ ...r, available: backup?.available || enabled });
      if (enabled) await loadBackupVersions();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const importBookmarks = async (file: File) => {
    setBookmarkBusy(true);
    setOk(null);
    try {
      const parsed = JSON.parse(await file.text()) as unknown;
      const raw = Array.isArray(parsed)
        ? parsed
        : (parsed as { bookmarks?: unknown }).bookmarks;
      if (!Array.isArray(raw)) throw new Error("文件缺少 bookmarks 数组");
      const bookmarks = raw
        .map((item) =>
          typeof item === "string"
            ? { message_id: item }
            : { message_id: (item as { message_id?: string }).message_id },
        )
        .filter((item): item is { message_id: string } => !!item.message_id);
      const r = await api<{ imported: number; skipped: number; invalid: number }>(
        "/api/v1/me/bookmarks/import",
        { method: "POST", body: JSON.stringify({ bookmarks }) },
      );
      setOk(
        `导入完成：新增 ${r.imported} 条，已存在 ${r.skipped} 条，无效 ${r.invalid} 条。`,
      );
      await fetchBookmarks(bookmarkQ);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  const saveTags = async (messageId: string) => {
    const tags = [...new Set(
      tagDraft
        .split(/[,，]/)
        .map((t) => t.trim())
        .filter(Boolean),
    )].slice(0, 5);
    try {
      const r = await api<{ tags: string[] }>(
        `/api/v1/me/bookmarks/${messageId}/tags`,
        { method: "PUT", body: JSON.stringify({ tags }) },
      );
      setBookmarks((prev) =>
        prev.map((b) => (b.message_id === messageId ? { ...b, tags: r.tags } : b)),
      );
      setTagEditId(null);
      setTagDraft("");
      await loadBookmarkTags();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const renameTag = async () => {
    if (!manageTag || !tagRename.trim()) return;
    setBookmarkBusy(true);
    try {
      await api(`/api/v1/me/bookmarks/tags/${encodeURIComponent(manageTag)}`, {
        method: "PUT",
        body: JSON.stringify({ name: tagRename }),
      });
      setManageTag("");
      setTagRename("");
      await fetchBookmarks(bookmarkQ);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  const removeTag = async () => {
    if (!manageTag) return;
    setBookmarkBusy(true);
    try {
      await api(`/api/v1/me/bookmarks/tags/${encodeURIComponent(manageTag)}`, {
        method: "DELETE",
      });
      setManageTag("");
      await fetchBookmarks(bookmarkQ);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  const batchTagSelected = async (remove: boolean) => {
    if (bookmarkSelected.size === 0 || !batchTag.trim()) return;
    setBookmarkBusy(true);
    try {
      await api("/api/v1/me/bookmarks/tags/batch", {
        method: "POST",
        body: JSON.stringify({
          message_ids: [...bookmarkSelected],
          tag: batchTag,
          remove,
        }),
      });
      await fetchBookmarks(bookmarkQ);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  const deleteSelectedBookmarks = async () => {
    const ids = [...bookmarkSelected];
    if (ids.length === 0) return;
    setBookmarkBusy(true);
    try {
      await api("/api/v1/me/bookmarks/batch-delete", {
        method: "POST",
        body: JSON.stringify({ message_ids: ids }),
      });
      setBookmarks((prev) => prev.filter((b) => !bookmarkSelected.has(b.message_id)));
      setBookmarkSelected(new Set());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBookmarkBusy(false);
    }
  };

  useEffect(() => {
    void load();
    void loadBitrateTiers()
      .then(setBitrateTiers)
      .catch(() => undefined);
  }, []);
  // 按账号角色过滤可用码率档位（房主可设档位由配置决定）。
  const roleRank = (role: string | undefined): number =>
    role === "super_admin" ? 3 : role === "admin" ? 2 : role === "user" ? 1 : 0;
  const availableTiers = bitrateTiers.filter(
    (t) => roleRank(me?.user.role) >= roleRank(t.min_owner_role),
  );

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setOk(null);
    try {
      const res = await api<{ auto_approved?: boolean; room?: { code: string } }>(
        "/api/v1/room-requests",
        {
          method: "POST",
          body: JSON.stringify({
            kind: "create_room",
            note: note || undefined,
            media_mode: mediaMode,
            audio_bitrate: mediaMode === "sfu" ? audioBitrate : 0,
            attachments: wantAttachments,
            desired_code:
              me?.user.role === "super_admin" && desiredCode.trim()
                ? normalizeRoomCode(desiredCode)
                : undefined,
          }),
        }
      );
      setNote("");
      setDesiredCode("");
      if (res.auto_approved && res.room) {
        window.location.href = `/${res.room.code}`;
        return;
      }
      setOk("申请已提交，等待管理员审批。");
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const cancel = async (id: string) => {
    try {
      await api(`/api/v1/room-requests/${id}/cancel`, { method: "POST", body: "{}" });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const search = async (e: FormEvent) => {
    e.preventDefault();
    const q = searchQ.trim();
    if (!q) {
      setSearchHits([]);
      setSearchDone(false);
      return;
    }
    setSearchBusy(true);
    setSearchError(null);
    try {
      const r = await api<{ results: SearchHit[] }>(
        `/api/v1/search?q=${encodeURIComponent(q)}&limit=20`,
      );
      setSearchHits(r.results);
      setSearchDone(true);
    } catch (err) {
      setSearchError(err instanceof Error ? err.message : String(err));
    } finally {
      setSearchBusy(false);
    }
  };

  const inboxRoom = (code: string) => inbox?.rooms.find((r) => r.room_code === code);

  return (
    <div className="page">
      <div className="page-head">
        <h1>我的房间</h1>
        <button className="link" onClick={() => void load()}>刷新</button>
      </div>
      {error && <p className="error">{error}</p>}
      {ok && <p className="ok">{ok}</p>}
      {inbox && (inbox.total_unread > 0 || inbox.total_mentions > 0) && (
        <section className="card">
          <h2>收件箱</h2>
          <p className="hint">
            {inbox.total_unread} 条未读 · {inbox.total_mentions} 条提及
          </p>
          <ul className="room-list">
            {inbox.rooms
              .filter((r) => r.unread + r.thread_unread + r.mentions > 0)
              .map((r) => (
                <li key={r.room_code}>
                  <Link to={`/${r.room_code}`} className="room-row">
                    <span className="code">{r.room_code}</span>
                    {r.unread > 0 && <span className="tag warn">{r.unread} 未读</span>}
                    {r.thread_unread > 0 && <span className="tag warn">{r.thread_unread} 线程</span>}
                    {r.mentions > 0 && <span className="tag warn">@{r.mentions}</span>}
                  </Link>
                </li>
              ))}
          </ul>
          {inbox.mentions.length > 0 && (
            <>
              <h3>最近提及</h3>
              <ul className="room-list">
                {inbox.mentions.map((m) => (
                  <li key={m.message_id}>
                    <Link
                      to={`/${m.room_code}${m.thread_root_id ? `?thread=${encodeURIComponent(m.thread_root_id)}` : ""}`}
                      className="room-row"
                    >
                      <span className="code">{m.room_code}</span>
                      <span className="muted">{m.sender_nickname || "成员"}</span>
                      <span className="search-snippet">{m.body}</span>
                      {m.unread && <span className="tag warn">未读</span>}
                    </Link>
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
      )}

      {(bookmarks.length > 0 || bookmarkQ.trim()) && (
        <section className="card">
          <h2>我的收藏</h2>
          <div className="row-actions bookmark-tools">
            <input
              value={bookmarkQ}
              onChange={(e) => setBookmarkQ(e.target.value)}
              placeholder="搜索收藏正文"
              maxLength={100}
              aria-label="搜索收藏"
            />
            <select
              value={tagFilter}
              onChange={(e) => setTagFilter(e.target.value)}
              aria-label="按标签筛选"
            >
              <option value="">全部标签</option>
              {bookmarkTags.map((t) => (
                <option key={t.tag} value={t.tag}>
                  {t.tag}（{t.count}）
                </option>
              ))}
            </select>
            {bookmarkTags.length > 0 && (
              <>
                <select
                  value={manageTag}
                  onChange={(e) => setManageTag(e.target.value)}
                  aria-label="管理标签"
                >
                  <option value="">管理标签</option>
                  {bookmarkTags.map((t) => (
                    <option key={t.tag} value={t.tag}>{t.tag}</option>
                  ))}
                </select>
                <input
                  value={tagRename}
                  onChange={(e) => setTagRename(e.target.value)}
                  placeholder="新名称"
                  aria-label="标签新名称"
                />
                <button
                  type="button"
                  className="link"
                  disabled={bookmarkBusy || !manageTag || !tagRename.trim()}
                  onClick={() => void renameTag()}
                >
                  重命名
                </button>
                <button
                  type="button"
                  className="link danger"
                  disabled={bookmarkBusy || !manageTag}
                  onClick={() => void removeTag()}
                >
                  删除标签
                </button>
              </>
            )}
            <input
              value={batchTag}
              onChange={(e) => setBatchTag(e.target.value)}
              placeholder="批量标签"
              aria-label="批量标签"
            />
            <button
              type="button"
              className="link"
              disabled={bookmarkBusy || bookmarkSelected.size === 0 || !batchTag.trim()}
              onClick={() => void batchTagSelected(false)}
            >
              加标签
            </button>
            <button
              type="button"
              className="link"
              disabled={bookmarkBusy || bookmarkSelected.size === 0 || !batchTag.trim()}
              onClick={() => void batchTagSelected(true)}
            >
              移除标签
            </button>
            <button
              type="button"
              className="link"
              disabled={bookmarkBusy || bookmarkSelected.size === 0}
              onClick={() => void deleteSelectedBookmarks()}
            >
              删除选中（{bookmarkSelected.size}）
            </button>
            <select
              value={exportFormat}
              onChange={(e) => setExportFormat(e.target.value as "json" | "csv" | "md")}
              aria-label="导出格式"
            >
              <option value="json">JSON</option>
              <option value="csv">CSV</option>
              <option value="md">Markdown</option>
            </select>
            <button
              type="button"
              className="link"
              disabled={bookmarkBusy || bookmarks.length === 0}
              onClick={() => void exportBookmarks()}
            >
              导出收藏
            </button>
            <label className="link">
              导入收藏
              <input
                type="file"
                accept=".json,application/json"
                hidden
                onChange={(e) => {
                  const file = e.target.files?.[0];
                  e.target.value = "";
                  if (file) void importBookmarks(file);
                }}
              />
            </label>
            {backup && (
              <label className="check">
                <input
                  type="checkbox"
                  checked={backup.enabled}
                  onChange={(e) => void toggleBackup(e.target.checked)}
                />
                自动备份（每周）
              </label>
            )}
            {backupVersions.length > 0 && (
              <>
                <select
                  value={backupVersion}
                  onChange={(e) => setBackupVersion(e.target.value)}
                  aria-label="备份版本"
                >
                  {backupVersions.map((v) => (
                    <option key={v.id} value={v.id}>
                      {new Date(v.created_at).toLocaleString()}
                    </option>
                  ))}
                </select>
                <a
                  className="link"
                  href={`${apiOrigin()}/api/v1/me/bookmarks/backup/versions/${encodeURIComponent(backupVersion)}`}
                  download
                >
                  下载版本
                </a>
                <button
                  type="button"
                  className="link"
                  disabled={bookmarkBusy || !backupVersion}
                  onClick={() => void restoreBackup()}
                >
                  恢复
                </button>
              </>
            )}
          </div>
          {bookmarks.length === 0 && <p className="hint">没有匹配的收藏。</p>}
          <ul className="room-list">
            {bookmarks.map((b) => (
              <li key={b.message_id} className="bookmark-row">
                <input
                  type="checkbox"
                  aria-label={`选择收藏 ${b.message_id}`}
                  checked={bookmarkSelected.has(b.message_id)}
                  onChange={(e) => {
                    setBookmarkSelected((prev) => {
                      const next = new Set(prev);
                      if (e.target.checked) next.add(b.message_id);
                      else next.delete(b.message_id);
                      return next;
                    });
                  }}
                />
                <Link
                  to={`/${b.room_code}?msg=${b.message_id}&seq=${b.seq}`}
                  className="room-row"
                >
                  <span className="code">{b.room_code}</span>
                  <span className="muted">{b.sender_nickname || b.sender_username || "成员"}</span>
                  <span className="search-snippet">{b.body}</span>
                  <span className="muted">{new Date(b.created_at).toLocaleString()}</span>
                </Link>
                <span className="bookmark-tags">
                  {(b.tags ?? []).map((tag) => (
                    <span key={tag} className="tag">
                      {tag}
                    </span>
                  ))}
                  {tagEditId === b.message_id ? (
                    <>
                    <input
                      autoFocus
                      value={tagDraft}
                      onChange={(e) => setTagDraft(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") void saveTags(b.message_id);
                        if (e.key === "Escape") setTagEditId(null);
                      }}
                      placeholder="标签，逗号分隔"
                      aria-label={`标签 ${b.message_id}`}
                    />
                    <button type="button" className="link" onClick={() => void saveTags(b.message_id)}>
                      保存
                    </button>
                    </>
                  ) : (
                    <button
                      type="button"
                      className="link"
                      onClick={() => {
                        setTagEditId(b.message_id);
                        setTagDraft((b.tags ?? []).join(", "));
                      }}
                    >
                      标签
                    </button>
                  )}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      {rooms.length === 0 && <p className="hint">还没有房间——在下方提交申请。</p>}
      <ul className="room-list">
        {rooms.map((r) => (
          <li key={r.code}>
            <Link to={`/${r.code}`} className="room-row">
              <span className="code">{r.code}</span>
              {(inboxRoom(r.code)?.unread ?? 0) > 0 && (
                <span className="tag warn">{inboxRoom(r.code)?.unread} 未读</span>
              )}
              {(inboxRoom(r.code)?.mentions ?? 0) > 0 && (
                <span className="tag warn">@{inboxRoom(r.code)?.mentions}</span>
              )}
              {(inboxRoom(r.code)?.thread_unread ?? 0) > 0 && (
                <span className="tag warn">{inboxRoom(r.code)?.thread_unread} 线程</span>
              )}
              <span className="tag">{r.kind === "direct" ? "私聊" : "房间"}</span>
              <span className="tag">
                {r.media_mode === "p2p" ? "P2P" : "SFU"}
              </span>
              <span className={`tag ${r.state === "active" ? "ok" : "warn"}`}>
                {r.state === "active" ? "正常" : r.state === "deleted" ? "已删除" : "已过期"}
              </span>
              <span className="muted">{r.my_role === "owner" ? "房主" : "成员"}</span>
            </Link>
          </li>
        ))}
      </ul>

      <section className="card">
        <h2>搜索所有房间</h2>
        <p className="hint">只搜索你当前加入的正常房间；结果最新在前。</p>
        <form onSubmit={search}>
          <label>
            关键词
            <input
              value={searchQ}
              onChange={(e) => setSearchQ(e.target.value)}
              placeholder="至少 1 个字符"
              maxLength={100}
            />
          </label>
          <button disabled={searchBusy}>{searchBusy ? "搜索中…" : "搜索"}</button>
        </form>
        {searchError && <p className="error">{searchError}</p>}
        {searchDone && searchHits.length === 0 && <p className="hint">没有匹配消息。</p>}
        <ul className="room-list">
          {searchHits.map((h) => (
            <li key={h.message_id}>
              <Link to={`/${h.room_code}`} className="room-row">
                <span className="code">{h.room_code}</span>
                <span className="muted">{h.sender_nickname || "成员"}</span>
                <span className="search-snippet">{h.body}</span>
                <span className="muted">{new Date(h.created_at).toLocaleString()}</span>
              </Link>
            </li>
          ))}
        </ul>
      </section>

      {me?.user.role === "guest" && (
        <section className="card">
          <h2>访客账号</h2>
          <p className="hint">访客不能申请建房，注册后即可申请。</p>
        </section>
      )}
      {me?.user.role !== "guest" && (
      <section className="card">
        <h2>申请创建房间</h2>
        <p className="hint">创建房间需选择媒体模式，提交后等待管理员审批。</p>
        <form onSubmit={submit}>
          <label>
            媒体模式
            <select value={mediaMode} onChange={(e) => setMediaMode(e.target.value as "p2p" | "sfu")}>
              <option value="p2p">P2P（浏览器直连）</option>
              <option value="sfu">SFU（LiveKit 转发）</option>
            </select>
          </label>
          {mediaMode === "sfu" && (
            <label>
              音频码率
              <select value={audioBitrate} onChange={(e) => setAudioBitrate(Number(e.target.value))}>
                {availableTiers.map((t) => (
                  <option key={t.kbps} value={t.kbps}>
                    {t.kbps} kbps
                  </option>
                ))}
              </select>
            </label>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={wantAttachments}
              onChange={(e) => setWantAttachments(e.target.checked)}
            />
            <span>
              允许附件（截图/文件）
              <small>需在申请时提出；开启后房间有独立容量配额，普通成员只能发截图。</small>
            </span>
          </label>
          <label>
            用途 / 说明
            <textarea
              value={note}
              onChange={(e) => setNote(e.target.value)}
              maxLength={500}
              placeholder="这个房间用来做什么？"
            />
          </label>
          {me?.user.role === "super_admin" && (
            <label>
              自定义房间号（仅超级管理员）
              <input
                value={desiredCode}
                onChange={(e) => setDesiredCode(e.target.value)}
                placeholder="例如 666 或 ccc — 3-32 位，仅 a-z0-9-"
              />
            </label>
          )}
          <button disabled={busy}>{busy ? "提交中…" : "提交申请"}</button>
        </form>
      </section>
      )}

      {requests.length > 0 && (
        <section>
          <h2>我的申请</h2>
          <ul className="room-list">
            {requests.map((r) => (
              <li key={r.request_id}>
                <span className="tag">{r.status}</span>
                <span className="tag">
                  {r.kind === "create_room" ? "创建房间" : "媒体模式变更"}
                </span>
                <span className="tag">{r.media_mode === "p2p" ? "P2P" : "SFU"}</span>
                {r.kind === "create_room" && r.desired_code && (
                  <span className="tag">code: {r.desired_code}</span>
                )}
                {r.kind === "change_media_mode" && r.target_room_code && (
                  <span className="tag">房间: {r.target_room_code}</span>
                )}{" "}
                {r.note || "（无说明）"}
                {r.status === "pending" && (
                  <button className="link" onClick={() => void cancel(r.request_id)}>
                    取消
                  </button>
                )}
                {r.status === "rejected" && r.decision_reason && (
                  <span className="muted"> — {r.decision_reason}</span>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
