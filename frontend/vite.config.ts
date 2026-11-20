import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { noiseSuppressionAudioWorkletVitePlugin } from "@workadventure/noise-suppression/vite";
// API origin for the backend. Same-origin by default (Nginx serves both);
// override with VITE_API_ORIGIN for cross-origin development.
const apiOrigin = process.env.VITE_API_ORIGIN || "";
/**
 * Replace the `__API_ORIGIN__` / `__API_WS_ORIGIN__` placeholders in the
 * static `_headers` file. CSP must allow the API and WebSocket origins for
 * cross-origin deployments, but the source tree must not hardcode one
 * deployment's address.
 */
function cspHeadersPlugin(): Plugin {
  let outDir = "dist";
  return {
    name: "cloudroom-csp-headers",
    apply: "build",
    configResolved(config) {
      outDir = config.build.outDir;
    },
    async closeBundle() {
      const file = resolve(process.cwd(), outDir, "_headers");
      const text = await readFile(file, "utf8");
      const origin = apiOrigin.trim().replace(/\/+$/, "");
      const wsOrigin = origin
        .replace(/^http:/, "ws:")
        .replace(/^https:/, "wss:");
      const rendered = text
        .replaceAll("__API_ORIGIN__", origin)
        .replaceAll("__API_WS_ORIGIN__", wsOrigin)
        .replace(/[ \t]+;/g, ";")
        .replace(/ {2,}/g, " ");
      await writeFile(file, rendered);
    },
  };
}
export default defineConfig({
  plugins: [react(), noiseSuppressionAudioWorkletVitePlugin(), cspHeadersPlugin()],
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target: apiOrigin || "http://127.0.0.1:8080",
        changeOrigin: false,
        ws: true,
      },
    },
  },
  build: {
    outDir: "dist",
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return;
          if (
            id.includes("/node_modules/livekit-client/") ||
            id.includes("/node_modules/@livekit/")
          ) {
            return "livekit";
          }
          if (
            id.includes("/node_modules/react/") ||
            id.includes("/node_modules/react-dom/") ||
            id.includes("/node_modules/react-router/") ||
            id.includes("/node_modules/react-router-dom/") ||
            id.includes("/node_modules/@remix-run/router/") ||
            id.includes("/node_modules/scheduler/")
          ) {
            return "react-vendor";
          }
        },
      },
    },
  },
});
