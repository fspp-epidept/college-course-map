<script setup lang="ts">
import { computed } from "vue";
import { activityById } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// The activity bar drives which activity (and therefore which sidebar
// component) is visible. Width is fixed for now; a resizable handle + per-user
// width persisted in the workspace store is a polish follow-up.
const active = computed(() => activityById(workspace.activeActivityId));
</script>

<template>
  <aside
    v-if="workspace.sidebarOpen"
    class="w-64 shrink-0 flex flex-col border-r border-(--ui-border) bg-(--ui-bg-elevated) overflow-hidden"
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
