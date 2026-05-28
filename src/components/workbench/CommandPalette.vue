<script setup lang="ts">
import { computed } from "vue";
import { activities, type ActivityId } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

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
]);
</script>

<template>
  <UDashboardSearch :groups="groups" placeholder="Jump to activity or open tab" />
</template>
