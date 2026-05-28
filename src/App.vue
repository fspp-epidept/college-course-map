<script setup lang="ts">
import AppTitleBar from "./components/AppTitleBar.vue";
import ActivityBar from "./components/workbench/ActivityBar.vue";
import CommandPalette from "./components/workbench/CommandPalette.vue";
import MainPanel from "./components/workbench/MainPanel.vue";
import PrimarySidebar from "./components/workbench/PrimarySidebar.vue";

// macOS keeps native chrome (decorations + global menu); Windows/Linux get the
// custom titlebar. See decision #102.
const isMacOS = import.meta.env.TAURI_ENV_PLATFORM === "macos";
</script>

<template>
  <UApp>
    <div class="h-screen flex flex-col">
      <AppTitleBar v-if="!isMacOS" />

      <!-- Workbench: VS-Code-style frame.
           ActivityBar (never collapses) | PrimarySidebar (per activity) | MainPanel.
           UDashboardGroup persists the resizable sidebar's width/collapsed state
           to localStorage under storageKey "dashboard". -->
      <div class="flex flex-1 min-h-0">
        <ActivityBar />

        <UDashboardGroup unit="rem" storage="local" class="flex flex-1 min-w-0">
          <PrimarySidebar />
          <MainPanel />
          <!-- Cmd/Ctrl-K opens the palette (UDashboardSearch's default shortcut).
               Mounted inside the group so it picks up dashboard context. -->
          <CommandPalette />
        </UDashboardGroup>
      </div>
    </div>
  </UApp>
</template>
