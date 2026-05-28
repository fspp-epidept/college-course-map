<script setup lang="ts">
import { computed } from "vue";
import { activityById } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// The activity bar drives which activity (and therefore which sidebar
// component) is visible. Width/resize persistence is handled by the parent
// UDashboardSidebar via UDashboardGroup's `storage="local"`.
const active = computed(() => activityById(workspace.activeActivityId));
</script>

<template>
  <UDashboardSidebar
    v-if="workspace.sidebarOpen"
    id="primary"
    resizable
    :default-size="18"
    :min-size="14"
    :max-size="36"
    class="bg-(--ui-bg-muted)/40"
  >
    <template #header>
      <div class="px-3 py-2 text-xs uppercase tracking-wide text-(--ui-text-dimmed)">
        {{ active?.label ?? "" }}
      </div>
    </template>

    <template #default>
      <component :is="active.sidebar" v-if="active" />
    </template>
  </UDashboardSidebar>
</template>
