<script setup lang="ts">
import { onMounted } from "vue";
import { commands } from "../../bindings";
import { useBoot } from "../../composables/useBoot";
import { useClassifyLifecycleRefresh } from "../../composables/useClassify";
import { useNativeMenu } from "../../composables/useNativeMenu";
import { useWorkspace } from "../../stores/workspace";
import DeriveDatasetDialog from "../../views/datasets/DeriveDatasetDialog.vue";
import ImportCsvDialog from "../ImportCsvDialog.vue";
import ActivityBar from "./ActivityBar.vue";
import CommandPalette from "./CommandPalette.vue";
import MainPanel from "./MainPanel.vue";
import PrimarySidebar from "./PrimarySidebar.vue";
import ResizeHandle from "./ResizeHandle.vue";

// Everything here queries the database or acts on it, so App.vue mounts
// this only once startup is ready (#224). Until then nothing listens for
// menu actions, so native menu items and shortcuts are inert by construction.

const workspace = useWorkspace();

// Refreshes courses/coverage/metrics when any classification ends, even if
// that dataset's page isn't mounted.
useClassifyLifecycleRefresh();

// Every macOS native menu item routes through here (docs/keybinds.md).
useNativeMenu();

// The previous session ended without a clean exit (#208): say so once,
// pointing at the log, which still holds that session's last lines.
const boot = useBoot();
const toast = useToast();
onMounted(() => {
  if (!boot.state.value?.uncleanExit) return;
  toast.add({
    title: "The app closed unexpectedly last time",
    description: "The log file has the details.",
    color: "warning",
    duration: 0,
    actions: [
      {
        label: "Open Logs Folder",
        icon: "i-lucide-folder-open",
        onClick: async () => {
          const result = await commands.openLogsDir();
          if (result.status === "error") {
            toast.add({
              title: "Couldn't open the logs folder",
              description: result.error,
              color: "error",
            });
          }
        },
      },
    ],
  });
});

// Cmd/Ctrl-B sidebar toggle. On macOS the native menu accelerator owns the
// keypress (Layer 2 — see docs/keybinds.md) and arrives via useNativeMenu, so
// the WebView binding is registered only on Windows/Linux, where there is no
// native menu (#104). Never both: a key seen by both layers would toggle twice.
if (import.meta.env.TAURI_ENV_PLATFORM !== "macos") {
  defineShortcuts({
    meta_b: () => workspace.toggleSidebar(),
  });
}
</script>

<template>
  <!--
    Workbench shell: a flex row of ActivityBar | PrimarySidebar | MainPanel.
    We don't use UDashboardGroup — its base class is `fixed inset-0`, which
    would pop the workbench out of this normal flow and overlay the titlebar
    + activity bar. UDashboardSidebar is similarly out (responsive
    `hidden lg:flex` collapses it under 1024px, always mounts a mobile
    slideover overlay). A plain <aside> works.
  -->
  <div class="flex flex-1 min-h-0">
    <ActivityBar />
    <PrimarySidebar />
    <ResizeHandle v-if="workspace.sidebarOpen" />
    <MainPanel />
  </div>

  <!-- Cmd/Ctrl-K: the native menu accelerator on macOS, UDashboardSearch's
       own defineShortcuts binding on Windows/Linux (see CommandPalette). -->
  <CommandPalette />

  <!-- Mounted here so the File menu can open it from any activity. -->
  <ImportCsvDialog v-model:open="workspace.importDialogOpen" />
  <DeriveDatasetDialog v-model:open="workspace.deriveDialogOpen" />
</template>
