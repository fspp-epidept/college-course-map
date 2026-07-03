<script setup lang="ts">
import { computed } from "vue";
import { activityById, tabKindPanels } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";
import TabStrip from "./TabStrip.vue";

const workspace = useWorkspace();

const activity = computed(() => activityById(workspace.activeActivityId));
const tab = computed(() => workspace.activeTab);
const tabBody = computed(() => (tab.value ? tabKindPanels[tab.value.kind] : null));
</script>

<template>
  <TabStrip />

  <div class="flex-1 min-h-0 overflow-auto">
    <!-- :key forces a fresh component instance per tab. Without it, Vue reuses
         one instance across all tabs of the same kind, and component-local
         state (pagination cursors, view level, error banners) silently bleeds
         from one dataset to the next (EPI-68). -->
    <component :is="tabBody" v-if="tab && tabBody" :key="tab.id" :tab="tab" />

    <!-- Empty state when no tabs are open in this activity. Quietly informs;
         doesn't try to drive an action until we know which CTA is right. -->
    <div
      v-else
      class="h-full flex flex-col items-center justify-center text-center gap-2 text-(--ui-text-dimmed)"
    >
      <UIcon name="i-lucide-square-dashed" class="size-10" />
      <p class="text-sm">
        Nothing open in <span class="text-(--ui-text-muted)">{{ activity?.label }}</span> yet.
      </p>
      <p class="text-xs">Pick something from the sidebar to open a tab.</p>
    </div>
  </div>
</template>
