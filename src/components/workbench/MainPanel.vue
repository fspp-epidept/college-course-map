<script setup lang="ts">
import { computed } from "vue";
import { activityById } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";
import TabbedView from "./TabbedView.vue";

const workspace = useWorkspace();
const activity = computed(() => activityById(workspace.activeActivityId));
</script>

<template>
  <section
    class="flex-1 min-w-0 flex flex-col bg-(--ui-bg)"
    aria-label="Main view"
  >
    <template v-if="activity">
      <!-- Tabbed activities (Datasets, Runs) render a tab strip + the active
           tab's body. Fixed activities (Overview, Models, Settings) render their
           single panel component. The modality decision (chat): tab state for
           inactive activities is preserved in the workspace store, not in DOM. -->
      <TabbedView v-if="activity.kind === 'tabbed'" />
      <component :is="activity.panel" v-else-if="activity.panel" />
    </template>
  </section>
</template>
