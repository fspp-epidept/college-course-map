import ui from "@nuxt/ui/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue(), ui()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  // 4. expose Tauri-injected env vars (TAURI_ENV_PLATFORM, TAURI_ENV_DEBUG, ...) to import.meta.env
  envPrefix: ["VITE_", "TAURI_ENV_*"],

  build: {
    // 5. Tauri ships into the system webview — pin the floor target accordingly
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    // 6. don't minify debug builds (readable stack traces in `tauri dev` / `tauri build --debug`)
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    // 7. produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
