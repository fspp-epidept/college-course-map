import "./assets/css/main.css";

import ui from "@nuxt/ui/vue-plugin";
import { createApp } from "vue";
import App from "./App.vue";
import { bootstrapTheme } from "./composables/useTheme";
import { router } from "./router";

// Apply the persisted theme before first paint so there is no flash of the wrong
// theme (FOUC). bootstrapTheme never rejects — it falls back to a built-in default.
async function bootstrap(): Promise<void> {
  await bootstrapTheme();
  createApp(App).use(router).use(ui).mount("#app");
}

void bootstrap();
