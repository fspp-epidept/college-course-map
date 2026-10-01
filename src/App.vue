<script setup lang="ts">
import AppTitleBar from "./components/AppTitleBar.vue";
import ImportCsvDialog from "./components/ImportCsvDialog.vue";
import ActivityBar from "./components/workbench/ActivityBar.vue";
import CommandPalette from "./components/workbench/CommandPalette.vue";
import MainPanel from "./components/workbench/MainPanel.vue";
import PrimarySidebar from "./components/workbench/PrimarySidebar.vue";
import ResizeHandle from "./components/workbench/ResizeHandle.vue";
import { useNativeMenu } from "./composables/useNativeMenu";
import { useRunLifecycleRefresh } from "./composables/useRuns";
import { useWorkspace } from "./stores/workspace";

const workspace = useWorkspace();

// Global run heartbeat: refreshes courses/coverage/datasets/metrics when any
// run finishes, even if the tab that started it is no longer mounted (EPI-68).
useRunLifecycleRefresh();

// macOS keeps native chrome (decorations + global menu); Windows/Linux get the
// custom titlebar. See decision #102.
const isMacOS = import.meta.env.TAURI_ENV_PLATFORM === "macos";

// Every macOS native menu item routes through here (docs/keybinds.md).
useNativeMenu();

// Cmd/Ctrl-B sidebar toggle. On macOS the native menu accelerator owns the
// keypress (Layer 2 — see docs/keybinds.md) and arrives via useNativeMenu, so
// the WebView binding is registered only on Windows/Linux, where there is no
// native menu (#104). Never both: a key seen by both layers would toggle twice.
if (!isMacOS) {
  defineShortcuts({
    meta_b: () => workspace.toggleSidebar(),
  });
}
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

    <!-- Cmd/Ctrl-K: the native menu accelerator on macOS, UDashboardSearch's
         own defineShortcuts binding on Windows/Linux (see CommandPalette). -->
    <CommandPalette />

    <!-- Root-mounted so the File menu can open it from any activity. -->
    <ImportCsvDialog v-model:open="workspace.importDialogOpen" />
  </UApp>
</template>
