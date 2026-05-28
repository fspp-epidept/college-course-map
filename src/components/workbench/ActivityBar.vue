<script setup lang="ts">
import { computed } from "vue";
import { activities, type ActivityId } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Split into top and bottom so the Settings cog anchors to the bottom edge,
// matching VS Code's activity bar layout.
const topActivities = computed(() => activities.filter((activity) => !activity.pinToBottom));
const bottomActivities = computed(() => activities.filter((activity) => activity.pinToBottom));

function select(id: ActivityId): void {
  // Clicking the active activity toggles the primary sidebar (matches VS Code).
  if (workspace.activeActivityId === id) {
    workspace.toggleSidebar();
    return;
  }
  workspace.setActiveActivity(id);
  if (!workspace.sidebarOpen) workspace.toggleSidebar();
}
</script>

<template>
  <aside
    class="w-12 shrink-0 flex flex-col items-stretch bg-(--ui-bg-elevated) border-r border-(--ui-border)"
    aria-label="Activity bar"
  >
    <div class="flex flex-col">
      <button
        v-for="activity in topActivities"
        :key="activity.id"
        type="button"
        :aria-label="activity.label"
        :aria-pressed="workspace.activeActivityId === activity.id"
        :title="activity.label"
        class="relative h-12 flex items-center justify-center text-(--ui-text-muted) hover:text-(--ui-text-highlighted) focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-(--ui-primary)"
        :class="{
          'text-(--ui-text-highlighted)': workspace.activeActivityId === activity.id,
        }"
        @click="select(activity.id)"
      >
        <span
          v-if="workspace.activeActivityId === activity.id"
          class="absolute inset-y-0 left-0 w-0.5 bg-(--ui-primary)"
          aria-hidden="true"
        />
        <UIcon :name="activity.icon" class="size-5" />
      </button>
    </div>

    <div class="mt-auto flex flex-col">
      <button
        v-for="activity in bottomActivities"
        :key="activity.id"
        type="button"
        :aria-label="activity.label"
        :aria-pressed="workspace.activeActivityId === activity.id"
        :title="activity.label"
        class="relative h-12 flex items-center justify-center text-(--ui-text-muted) hover:text-(--ui-text-highlighted) focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-(--ui-primary)"
        :class="{
          'text-(--ui-text-highlighted)': workspace.activeActivityId === activity.id,
        }"
        @click="select(activity.id)"
      >
        <span
          v-if="workspace.activeActivityId === activity.id"
          class="absolute inset-y-0 left-0 w-0.5 bg-(--ui-primary)"
          aria-hidden="true"
        />
        <UIcon :name="activity.icon" class="size-5" />
      </button>
    </div>
  </aside>
</template>
