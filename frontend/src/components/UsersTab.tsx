import { useEffect, useState } from "react";
import { User, api } from "../api";
import { roleLabel } from "../lib/roles";

export interface UsersViewProps {
  users: User[];
  isSuper: boolean;
  error: string | null;
  temporaryPassword: string | null;
  onResetPassword: (userId: string) => void;
  onToggleDisabled: (userId: string, disabled: boolean) => void;
  onRoleChange: (userId: string, role: string) => void;
}

export function UsersView({
  users,
  isSuper,
  error,
  temporaryPassword,
  onResetPassword,
  onToggleDisabled,
  onRoleChange,
}: UsersViewProps) {
  return (
    <section>
      {error && <p className="error">{error}</p>}
      {temporaryPassword && (
        <p className="ok">
          临时密码（仅显示一次）：<code>{temporaryPassword}</code>
        </p>
      )}
      <div className="table-wrap">
        <table className="data-table">
        <thead>
          <tr>
            <th>用户名</th>
            <th>角色</th>
            <th>创建时间</th>
            <th>状态</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {users.map((u) => (
            <tr key={u.user_id}>
              <td>{u.username}</td>
              <td>{roleLabel(u.role)}</td>
              <td>{new Date(u.created_at).toLocaleString()}</td>
              <td>{u.disabled ? <span className="tag warn">已禁用</span> : <span className="tag ok">正常</span>}</td>
              <td className="row-actions">
                {isSuper && (
                  <button onClick={() => onResetPassword(u.user_id)}>重置密码</button>
                )}
                {u.role !== "super_admin" && (
                  <button onClick={() => onToggleDisabled(u.user_id, !u.disabled)}>
                    {u.disabled ? "启用" : "禁用"}
                  </button>
                )}
                {isSuper && u.role !== "super_admin" && (
                  <select value={u.role} onChange={(e) => onRoleChange(u.user_id, e.target.value)}>
                    <option value="user">普通用户</option>
                    <option value="admin">管理员</option>
                  </select>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
        </div>
    </section>
  );
}

export function UsersTab({ isSuper }: { isSuper: boolean }) {
  const [users, setUsers] = useState<User[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [tempPw, setTempPw] = useState<string | null>(null);

  const load = () => {
    api<{ users: User[] }>("/api/v1/admin/users")
      .then((r) => setUsers(r.users))
      .catch((e) => setError(e.message));
  };
  useEffect(load, []);

  const setDisabled = async (uid: string, disabled: boolean) => {
    try {
      await api(`/api/v1/admin/users/${uid}/disable`, {
        method: "POST",
        body: JSON.stringify({ disabled }),
      });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const resetPw = async (uid: string) => {
    if (!window.confirm("重置该用户密码？其所有会话将立即失效。")) return;
    try {
      const r = await api<{ temporary_password: string }>(`/api/v1/admin/users/${uid}/reset-password`, {
        method: "POST",
        body: "{}",
      });
      setTempPw(r.temporary_password);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const setRole = async (uid: string, role: string) => {
    try {
      await api(`/api/v1/admin/users/${uid}/role`, {
        method: "POST",
        body: JSON.stringify({ role }),
      });
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <UsersView
      users={users}
      isSuper={isSuper}
      error={error}
      temporaryPassword={tempPw}
      onResetPassword={(uid) => void resetPw(uid)}
      onToggleDisabled={(uid, disabled) => void setDisabled(uid, disabled)}
      onRoleChange={(uid, role) => void setRole(uid, role)}
    />
  );
}
