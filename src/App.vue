<script setup lang="ts">
import { listen } from "@tauri-apps/api/event";
import { onBeforeUnmount, onMounted } from "vue";
import AppTitleBar from "./components/AppTitleBar.vue";
import ActivityBar from "./components/workbench/ActivityBar.vue";
import CommandPalette from "./components/workbench/CommandPalette.vue";
import MainPanel from "./components/workbench/MainPanel.vue";
import PrimarySidebar from "./components/workbench/PrimarySidebar.vue";
import ResizeHandle from "./components/workbench/ResizeHandle.vue";
import { useRunLifecycleRefresh } from "./composables/useRuns";
import { useWorkspace } from "./stores/workspace";

const workspace = useWorkspace();

// Global run heartbeat: refreshes courses/coverage/datasets/metrics when any
// run finishes, even if the tab that started it is no longer mounted (EPI-68).
useRunLifecycleRefresh();

// macOS keeps native chrome (decorations + global menu); Windows/Linux get the
// custom titlebar. See decision #102.
const isMacOS = import.meta.env.TAURI_ENV_PLATFORM === "macos";

// Cmd/Ctrl-B sidebar toggle. On macOS the native menu accelerator intercepts the
// keypress (Layer 2 — see docs/keybinds.md) and emits `menu:toggle_sidebar`; the
// WebView never sees Cmd-B, so defineShortcuts is a no-op there. On Windows/Linux
// there is no native menu yet (#104), so Layer 3 carries the binding directly.
// Both paths call the same store action — duplication of effect, not of binding.
defineShortcuts({
  meta_b: () => workspace.toggleSidebar(),
});

let unlistenToggleSidebar: (() => void) | undefined;
onMounted(async () => {
  unlistenToggleSidebar = await listen("menu:toggle_sidebar", () => workspace.toggleSidebar());
});
onBeforeUnmount(() => {
  unlistenToggleSidebar?.();
});
</script>

<template>
  <UApp>
    <!--
      Workbench shell: AppTitleBar (Win/Linux) above a flex row of
      ActivityBar | PrimarySidebar | MainPanel. We don't use UDashboardGroup —
      its base class is `fixed inset-0`, which would pop the workbench out of
      this normal flow and overlay the titlebar + activity bar. UDashboardSidebar
      is similarly out (responsive `hidden lg:flex` collapses it under 1024px,
      always mounts a mobile slideover overlay). A plain <aside> works.
    -->
    <div class="h-screen flex flex-col overflow-hidden">
      <AppTitleBar v-if="!isMacOS" />

      <div class="flex flex-1 min-h-0">
        <ActivityBar />
        <PrimarySidebar />
        <ResizeHandle v-if="workspace.sidebarOpen" />
        <MainPanel />
      </div>
    </div>

    <!-- Cmd/Ctrl-K opens the palette via UDashboardSearch's own defineShortcuts
         binding (works standalone, no UDashboardGroup needed). -->
    <CommandPalette />
  </UApp>
</template>
