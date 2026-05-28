<script setup lang="ts">
import { computed } from "vue";
import { activities, type ActivityId } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

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

interface OpenTabRow {
  activityId: ActivityId;
  tabId: string;
  label: string;
  icon?: string;
  activityLabel: string;
}

// Surface every open tab in every activity as a quick-jump target — Cmd-K
// becomes the only thing you need to switch context.
const openTabRows = computed<OpenTabRow[]>(() => {
  const rows: OpenTabRow[] = [];
  for (const activity of activities) {
    const tabs = workspace.tabsByActivity[activity.id] ?? [];
    for (const tab of tabs) {
      rows.push({
        activityId: activity.id,
        tabId: tab.id,
        label: tab.label,
        icon: tab.icon,
        activityLabel: activity.label,
      });
    }
  }
  return rows;
});

function jumpToActivity(id: ActivityId): void {
  workspace.setActiveActivity(id);
  if (!workspace.sidebarOpen) workspace.toggleSidebar();
}

function jumpToTab(row: OpenTabRow): void {
  workspace.setActiveActivity(row.activityId);
  workspace.setActiveTab(row.activityId, row.tabId);
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
    id: "tabs",
    label: "Open tabs",
    items: openTabRows.value.map((row) => ({
      label: row.label,
      icon: row.icon,
      suffix: row.activityLabel,
      onSelect: () => jumpToTab(row),
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
