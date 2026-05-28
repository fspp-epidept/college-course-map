import "./assets/css/main.css";

import ui from "@nuxt/ui/vue-plugin";
import { VueQueryPlugin } from "@tanstack/vue-query";
import { createPinia } from "pinia";
import piniaPluginPersistedstate from "pinia-plugin-persistedstate";
import { createApp } from "vue";
import App from "./App.vue";
import { router } from "./router";
import { bootstrapTheme } from "./stores/theme";

async function bootstrap(): Promise<void> {
  const app = createApp(App);

  // Pinia must be installed before bootstrapTheme so the theme store resolves.
  // Persist opted-in stores to localStorage so workspace state (active activity,
  // open tabs) survives reloads. Opt-in per store via `persist: true`.
  const pinia = createPinia();
  pinia.use(piniaPluginPersistedstate);
  app.use(pinia);

  // Apply the persisted theme before first paint so there is no flash of the
  // wrong theme (FOUC). bootstrapTheme never rejects — it falls back to a
  // built-in default. Mount hasn't happened yet, so this is still pre-paint.
  await bootstrapTheme();

  // TanStack Query: aggressive defaults for a desktop app. IPC results are
  // deterministic for the same args, so cached data stays fresh until we
  // explicitly invalidate it on mutation. No window-focus refetch — a hidden
  // Tauri window isn't a meaningful "stale tab" signal.
  app.use(VueQueryPlugin, {
    queryClientConfig: {
      defaultOptions: {
        queries: {
          staleTime: Number.POSITIVE_INFINITY,
          refetchOnWindowFocus: false,
        },
      },
    },
  });

  app.use(router).use(ui).mount("#app");
}

void bootstrap();
