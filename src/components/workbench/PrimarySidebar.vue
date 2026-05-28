<script setup lang="ts">
import { computed } from "vue";
import { activityById } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

const active = computed(() => activityById(workspace.activeActivityId));

// Inline style: width is per-user (drag-resizable, persisted in the workspace
// store as rem). A static Tailwind class can't carry a dynamic value.
const widthStyle = computed(() => ({ width: `${workspace.sidebarWidthRem}rem` }));
</script>

<template>
  <aside
    v-if="workspace.sidebarOpen"
    :style="widthStyle"
    class="shrink-0 flex flex-col bg-(--ui-bg-muted) overflow-hidden"
    aria-label="Primary sidebar"
  >
    <header
      class="h-9 shrink-0 flex items-center px-3 text-xs uppercase tracking-wide text-(--ui-text-dimmed) border-b border-(--ui-border)"
    >
      {{ active?.label ?? "" }}
    </header>
    <div class="flex-1 min-h-0 overflow-y-auto">
      <component :is="active.sidebar" v-if="active" />
    </div>
  </aside>
</template>
