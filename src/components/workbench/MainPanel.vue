<script setup lang="ts">
import { computed } from "vue";
import { activityById } from "../../config/activities";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();
const activity = computed(() => activityById(workspace.activeActivityId));
</script>

<template>
  <section
    class="flex-1 min-w-0 flex flex-col bg-(--ui-bg)"
    aria-label="Main view"
  >
    <!-- One panel per activity (EPI-58 master/detail). Datasets and Runs
         render their sidebar selection's detail; selection lives in the
         workspace store, so switching activities and back restores it. -->
    <div v-if="activity" class="flex-1 min-h-0 overflow-auto">
      <component :is="activity.panel" />
    </div>
  </section>
</template>
