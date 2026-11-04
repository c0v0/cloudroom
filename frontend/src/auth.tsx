import React, { createContext, useContext, useEffect, useState } from "react";
import { MeResponse, api, me, invalidateMe } from "./api";

interface AuthCtx {
  me: MeResponse | null;
  loading: boolean;
  refresh: () => Promise<void>;
  logout: () => Promise<void>;
}

const Ctx = createContext<AuthCtx>({
  me: null,
  loading: true,
  refresh: async () => {},
  logout: async () => {},
});

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [meState, setMe] = useState<MeResponse | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = async () => {
    const m = await me();
    setMe(m);
    setLoading(false);
  };

  useEffect(() => {
    void refresh();
  }, []);

  // 任意 API 返回 401（会话过期/被撤销）时立刻退出登录态，避免 UI 卡住。
  useEffect(() => {
    const onUnauthorized = () => {
      setMe(null);
      setLoading(false);
    };
    window.addEventListener("cloudroom:unauthorized", onUnauthorized);
    const onStorage = (e: StorageEvent) => {
      if (e.key === "cloudroom:logout") {
        invalidateMe();
        setMe(null);
        setLoading(false);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener("cloudroom:unauthorized", onUnauthorized);
      window.removeEventListener("storage", onStorage);
    };
  }, []);

  const logout = async () => {
    try {
      await api("/api/v1/auth/logout", { method: "POST", body: "{}" });
    } catch (e) {
      // Never present a logged-out UI while the HttpOnly session is still
      // valid; keep the state and let the user retry.
      console.error("logout failed", e);
      return;
    }
    invalidateMe();
    setMe(null);
    try {
      localStorage.setItem("cloudroom:logout", String(Date.now()));
    } catch {
      /* storage may be unavailable in private mode */
    }
    location.href = "/login";
  };

  return (
    <Ctx.Provider value={{ me: meState, loading, refresh, logout }}>{children}</Ctx.Provider>
  );
}

export function useAuth(): AuthCtx {
  return useContext(Ctx);
}
