<script setup lang="ts">
import { computed } from "vue";
import { useDatasets } from "../../composables/useDatasets";
import { activities, type ActivityId } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Every dataset is a jump target: the palette reaches any of them, not only
// the one selected. The query shares its cache with the sidebar.
const { data: datasets } = useDatasets();

// Bind UDashboardSearch's open state to the workspace store so other surfaces
// (e.g. the centered search button in the titlebar) can toggle the same modal.
// UDashboardSearch's defineShortcuts handler mutates `open.value`, which writes
// through to the store ref — keyboard and button stay in sync.
const open = computed({
  get: () => workspace.commandPaletteOpen,
  set: (value) => {
    workspace.commandPaletteOpen = value;
  },
});

function jumpToActivity(id: ActivityId): void {
  workspace.setActiveActivity(id);
  if (!workspace.sidebarOpen) workspace.toggleSidebar();
}

function jumpToDataset(id: string): void {
  workspace.selectDataset(id);
  workspace.setActiveActivity("datasets");
}

function jumpToSettingsSection(section: "general" | "theme" | "about"): void {
  workspace.setActiveActivity("settings");
  workspace.setActiveSettingsSection(section);
  if (!workspace.sidebarOpen) workspace.toggleSidebar();
}

// Cmd/Ctrl-K: on macOS the native View menu accelerator owns it (Layer 2,
// docs/keybinds.md) and toggles the store via useNativeMenu, so the WebView
// binding is turned off there — an empty shortcut never matches a key.
// Windows/Linux keep UDashboardSearch's own "meta_k" binding.
const shortcut = import.meta.env.TAURI_ENV_PLATFORM === "macos" ? "" : "meta_k";

// UDashboardSearch consumes the result of this computed.
const groups = computed(() => [
  {
    id: "activities",
    label: "Go to",
    items: activities.map((activity) => ({
      label: activity.label,
      icon: activity.icon,
      onSelect: () => jumpToActivity(activity.id),
    })),
  },
  {
    id: "datasets",
    label: "Datasets",
    items: (datasets.value ?? []).map((dataset) => ({
      label: dataset.title,
      icon: "i-lucide-database",
      suffix: "Dataset",
      onSelect: () => jumpToDataset(dataset.id),
    })),
  },
  {
    id: "commands",
    label: "Commands",
    items: [
      {
        label: "Switch Theme…",
        icon: "i-lucide-palette",
        onSelect: () => jumpToSettingsSection("theme"),
      },
      {
        label: "Toggle Sidebar",
        icon: "i-lucide-panel-left",
        suffix: "Cmd/Ctrl-B",
        onSelect: () => workspace.toggleSidebar(),
      },
    ],
  },
]);
</script>

<template>
  <!-- :color-mode="false" suppresses UDashboardSearch's auto-added "Theme"
       group; theming is driven by settings.json + the registry (#106), not by
       VueUse's useColorMode preference. -->
  <UDashboardSearch
    v-model:open="open"
    :groups="groups"
    :color-mode="false"
    :shortcut="shortcut"
    placeholder="Jump to an activity or dataset"
  />
</template>
