import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// API origin for the backend. Same-origin by default (Nginx serves both);
// override with VITE_API_ORIGIN for cross-origin development.
const apiOrigin = process.env.VITE_API_ORIGIN || "";

export default defineConfig({
  plugins: [react()],
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
  },
});
