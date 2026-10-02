<script setup lang="ts">
import AppTitleBar from "./components/AppTitleBar.vue";
import BootScreen from "./components/BootScreen.vue";
import Workbench from "./components/workbench/Workbench.vue";
import { provideBoot } from "./composables/useBoot";

// macOS keeps native chrome (decorations + global menu); Windows/Linux get the
// custom titlebar. See decision #102.
const isMacOS = import.meta.env.TAURI_ENV_PLATFORM === "macos";

// Startup runs in the background (#224): the boot screen stands in for the
// workbench until the database and runtime are ready. A v-if, not an
// overlay, so nothing under the workbench queries before then.
const { screenVisible, workbenchReady } = provideBoot();
</script>

<template>
  <UApp>
    <div class="h-screen flex flex-col overflow-hidden">
      <AppTitleBar v-if="!isMacOS" />
      <Workbench v-if="workbenchReady" />
      <BootScreen v-else-if="screenVisible" />
    </div>
  </UApp>
</template>
