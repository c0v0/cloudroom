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

  const logout = async () => {
    try {
      await api("/api/v1/auth/logout", { method: "POST", body: "{}" });
    } catch {
      /* ignore */
    }
    invalidateMe();
    setMe(null);
    location.href = "/login";
  };

  return (
    <Ctx.Provider value={{ me: meState, loading, refresh, logout }}>{children}</Ctx.Provider>
  );
}

export function useAuth(): AuthCtx {
  return useContext(Ctx);
}
