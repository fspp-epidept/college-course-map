<script setup lang="ts">
import { RouterLink, RouterView } from "vue-router";
import AppTitleBar from "./components/AppTitleBar.vue";

// macOS keeps native chrome (decorations + global menu); Windows/Linux get the
// custom titlebar. See decision #102.
const isMacOS = import.meta.env.TAURI_ENV_PLATFORM === "macos";

const routes = [
  { to: "/datasets", label: "Datasets" },
  { to: "/runs", label: "Runs" },
  { to: "/models", label: "Models" },
  { to: "/settings", label: "Settings" },
];
</script>

<template>
  <UApp>
    <div class="min-h-screen flex flex-col">
      <AppTitleBar v-if="!isMacOS" />
      <nav class="border-b border-(--ui-border) px-6 py-3 flex gap-4">
        <RouterLink
          v-for="route in routes"
          :key="route.to"
          :to="route.to"
          class="text-sm text-(--ui-text-muted) hover:text-(--ui-text)"
          active-class="text-(--ui-primary) font-medium"
        >
          {{ route.label }}
        </RouterLink>
      </nav>
      <main class="flex-1">
        <RouterView />
      </main>
    </div>
  </UApp>
</template>
