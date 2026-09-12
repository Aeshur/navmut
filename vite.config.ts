import { defineConfig } from "vite";
// @ts-expect-error @types/node is not a project dependency.
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

export default defineConfig(() => ({
  // Keep Vite from clearing Rust errors.
  clearScreen: false,
  publicDir: false,
  // Tauri requires a fixed port. Fail if it is unavailable.
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
