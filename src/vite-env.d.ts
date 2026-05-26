/// <reference types="vite/client" />

interface ImportMetaEnv {
  // Injected by the Tauri CLI during dev/build (exposed via vite envPrefix).
  readonly TAURI_ENV_PLATFORM?: "macos" | "windows" | "linux" | (string & {});
}

declare module "*.vue" {
  import type { DefineComponent } from "vue";

  const component: DefineComponent<{}, {}, any>;
  export default component;
}
