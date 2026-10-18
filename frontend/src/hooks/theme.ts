/**
 * 主题切换：跟随系统 / 深色 / 浅色。偏好存 localStorage，
 * 应用时设置 documentElement 的 data-theme（styles.css 提供浅色变量组）。
 */

export type ThemeMode = "system" | "dark" | "light";

const KEY = "cloudroom.theme";

export function getThemeMode(): ThemeMode {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === "system" || raw === "dark" || raw === "light") return raw;
  } catch {
    /* 忽略 */
  }
  return "system";
}

export function setThemeMode(mode: ThemeMode): void {
  try {
    localStorage.setItem(KEY, mode);
  } catch {
    /* 忽略 */
  }
  applyTheme(mode);
}

export function applyTheme(mode: ThemeMode): void {
  const dark = mode === "dark" || (mode === "system" && !window.matchMedia("(prefers-color-scheme: light)").matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  document.documentElement.style.colorScheme = dark ? "dark" : "light";
}
