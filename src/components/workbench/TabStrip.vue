<script setup lang="ts">
import { type OpenTab, useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

function focus(tabId: string): void {
  workspace.setActiveTab(workspace.activeActivityId, tabId);
}

function close(event: Event, tabId: string): void {
  // Stop the click from also activating the tab we're closing.
  event.stopPropagation();
  workspace.closeTab(workspace.activeActivityId, tabId);
}

// VS Code-style tab context menu (EPI-71). Items are computed per right-
// clicked tab; the store owns all tab mutation — the menu only dispatches.
function menuItems(tab: OpenTab) {
  const activity = workspace.activeActivityId;
  const tabs = workspace.activeTabs;
  const index = tabs.findIndex((t) => t.id === tab.id);
  return [
    {
      label: "Close",
      onSelect: () => workspace.closeTab(activity, tab.id),
    },
    {
      label: "Close Others",
      disabled: tabs.length <= 1,
      onSelect: () => workspace.closeOtherTabs(activity, tab.id),
    },
    {
      label: "Close to the Right",
      disabled: index === tabs.length - 1,
      onSelect: () => workspace.closeTabsToRight(activity, tab.id),
    },
    {
      label: "Close All",
      onSelect: () => workspace.closeAllTabs(activity),
    },
  ];
}
</script>

<template>
  <div
    role="tablist"
    class="flex items-stretch h-9 border-b border-(--ui-border) bg-(--ui-bg-elevated) overflow-x-auto"
  >
    <!-- UContextMenu's trigger merges onto the slot root (Reka as-child), so
         the <button> stays the direct flex item and the native webview menu
         is suppressed on tabs only. -->
    <UContextMenu
      v-for="tab in workspace.activeTabs"
      :key="tab.id"
      :items="menuItems(tab)"
    >
      <button
        type="button"
        role="tab"
        :aria-selected="tab.id === workspace.activeTabId"
        class="group relative flex items-center gap-2 px-3 py-1.5 text-sm border-r border-(--ui-border) text-(--ui-text-muted) hover:text-(--ui-text-highlighted) hover:bg-(--ui-bg-muted) min-w-0"
        :class="{
          'bg-(--ui-bg) text-(--ui-text-highlighted)': tab.id === workspace.activeTabId,
        }"
        @click="focus(tab.id)"
      >
        <span
          v-if="tab.id === workspace.activeTabId"
          class="absolute inset-x-0 top-0 h-0.5 bg-(--ui-primary)"
          aria-hidden="true"
        />
        <UIcon v-if="tab.icon" :name="tab.icon" class="size-3.5 shrink-0" />
        <span class="truncate max-w-48">{{ tab.label }}</span>
        <UIcon
          name="i-lucide-x"
          class="size-3.5 shrink-0 opacity-0 group-hover:opacity-100 hover:text-(--ui-text-highlighted)"
          :class="{ 'opacity-100': tab.id === workspace.activeTabId }"
          role="button"
          aria-label="Close tab"
          tabindex="-1"
          @click="close($event, tab.id)"
        />
      </button>
    </UContextMenu>
  </div>
</template>
