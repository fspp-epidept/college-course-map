<script setup lang="ts">
import { computed } from "vue";
import { useDatasets } from "../../composables/useDatasets";
import { useRuns } from "../../composables/useRuns";
import { activities, type ActivityId } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Every dataset and run is a jump target (EPI-58) — selection-driven
// navigation replaced the old "open tabs" list, so the palette now reaches
// anything, not just what was already open. Both queries share their cache
// with the sidebars, and their polling is conditional on active work.
const { data: datasets } = useDatasets();
const { data: runs } = useRuns();

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

function jumpToRun(id: string): void {
  workspace.selectRun(id);
  workspace.setActiveActivity("runs");
}

function jumpToSettingsSection(section: "general" | "theme" | "about"): void {
  workspace.setActiveActivity("settings");
  workspace.setActiveSettingsSection(section);
  if (!workspace.sidebarOpen) workspace.toggleSidebar();
}

// UDashboardSearch consumes the result of this computed and binds Cmd/Ctrl-K
// itself (its `shortcut` prop defaults to "meta_k").
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
    id: "runs",
    label: "Runs",
    items: (runs.value ?? []).map((run) => ({
      label: `${run.datasetTitle} · ${run.id.slice(0, 8)}`,
      icon: "i-lucide-play",
      suffix: run.state,
      onSelect: () => jumpToRun(run.id),
    })),
  },
  {
    id: "commands",
    label: "Commands",
    items: [
      {
        label: "Switch theme…",
        icon: "i-lucide-palette",
        onSelect: () => jumpToSettingsSection("theme"),
      },
      {
        label: "Toggle sidebar",
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
    placeholder="Jump to activity or open tab"
  />
</template>
