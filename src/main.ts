import "./assets/css/main.css";

import ui from "@nuxt/ui/vue-plugin";
import { createApp } from "vue";
import App from "./App.vue";
import { router } from "./router";

createApp(App).use(router).use(ui).mount("#app");
