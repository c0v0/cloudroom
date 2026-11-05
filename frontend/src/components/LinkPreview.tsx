import { useEffect, useState } from "react";
import { api } from "../api";

interface Preview {
  url: string;
  title: string;
  description: string;
  site_name: string;
}

const cache = new Map<string, Preview | null>();

export function LinkPreview({ url }: { url: string }) {
  const [preview, setPreview] = useState<Preview | null>(() => cache.get(url) ?? null);
  const [done, setDone] = useState(() => cache.has(url));

  useEffect(() => {
    let alive = true;
    if (cache.has(url)) {
      setPreview(cache.get(url) ?? null);
      setDone(true);
      return;
    }
    api<{ preview: Preview }>(
      `/api/v1/link-preview?url=${encodeURIComponent(url)}`,
    )
      .then((r) => {
        cache.set(url, r.preview);
        if (alive) setPreview(r.preview);
      })
      .catch(() => {
        cache.set(url, null);
      })
      .finally(() => {
        if (alive) setDone(true);
      });
    return () => {
      alive = false;
    };
  }, [url]);

  if (!done || !preview || (!preview.title && !preview.description)) return null;
  return (
    <a
      className="link-preview"
      href={preview.url}
      target="_blank"
      rel="noopener noreferrer"
    >
      <b>{preview.title || preview.site_name || preview.url}</b>
      {preview.site_name && <span className="link-preview-site">{preview.site_name}</span>}
      {preview.description && <span className="link-preview-desc">{preview.description}</span>}
    </a>
  );
}
