import "./assets/css/main.css";

import ui from "@nuxt/ui/vue-plugin";
import { createPinia } from "pinia";
import piniaPluginPersistedstate from "pinia-plugin-persistedstate";
import { createApp } from "vue";
import App from "./App.vue";
import { bootstrapTheme } from "./composables/useTheme";
import { router } from "./router";

// Apply the persisted theme before first paint so there is no flash of the wrong
// theme (FOUC). bootstrapTheme never rejects — it falls back to a built-in default.
async function bootstrap(): Promise<void> {
  await bootstrapTheme();

  const pinia = createPinia();
  // Persist opted-in stores to localStorage so workspace state (active activity,
  // open tabs) survives reloads. Opt-in per store via `persist: true`.
  pinia.use(piniaPluginPersistedstate);

  createApp(App).use(pinia).use(router).use(ui).mount("#app");
}

void bootstrap();
