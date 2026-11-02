import { lazy, Suspense, useEffect, useState } from "react";
import { Link, Navigate, Route, Routes, useLocation } from "react-router-dom";
import { useAuth } from "./auth";
import { applyTheme, getThemeMode } from "./hooks/theme";
import { isRoomCodePath } from "./lib/roomCode";
import Home from "./pages/Home";
import Login from "./pages/Login";
import Register from "./pages/Register";

const RoomPage = lazy(() => import("./pages/Room"));
const Admin = lazy(() => import("./pages/Admin"));
const SettingsModal = lazy(() => import("./pages/Settings"));

// 非房间路径：settings 是弹窗入口，直链访问回落首页而不是被当成房间号。
const APP_ROUTES = new Set(["login", "register", "admin", "settings", ""]);

export default function App() {
  const { me, loading, logout } = useAuth();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const location = useLocation();
  const path = location.pathname.slice(1).split("/")[0];

  // 主题：应用持久化偏好；"跟随系统"时监听系统变化。
  useEffect(() => {
    applyTheme(getThemeMode());
    const mq = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => {
      if (getThemeMode() === "system") applyTheme("system");
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  if (loading) {
    return <div className="page center">加载中…</div>;
  }

  const isRoomCode = !APP_ROUTES.has(path) && isRoomCodePath(path);
  const isAdmin = me && (me.user.role === "admin" || me.user.role === "super_admin");

  return (
    <div className="app">
      <header className="topbar">
        <Link to="/" className="brand">
          云间 CloudRoom
        </Link>
        <nav>
          {me ? (
            <>
              {isAdmin ? <Link to="/admin">管理后台</Link> : null}
              <button className="link" onClick={() => setSettingsOpen(true)}>
                设置
              </button>
              <button className="link" onClick={() => void logout()}>
                退出登录
              </button>
            </>
          ) : (
            <>
              <Link to="/login">登录</Link>
              <Link to="/register">注册</Link>
            </>
          )}
        </nav>
      </header>
      <main>
        <Suspense fallback={null}>
          <Routes>
            <Route path="/" element={me ? <Home /> : <Navigate to="/login" />} />
            <Route path="/login" element={<Login />} />
            <Route path="/register" element={<Register />} />
            <Route
              path="/admin"
              element={me && isAdmin ? <Admin /> : <Navigate to="/" />}
            />
            <Route
              path="/:code"
              element={isRoomCode ? <RoomPage /> : <Navigate to="/" />}
            />
            <Route path="*" element={<Navigate to="/" />} />
          </Routes>
        </Suspense>
      </main>
      {settingsOpen && (
        <Suspense fallback={null}>
          <SettingsModal onClose={() => setSettingsOpen(false)} />
        </Suspense>
      )}
    </div>
  );
}
