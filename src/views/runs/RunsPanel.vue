<script setup lang="ts">
import { computed } from "vue";
import { useWorkspace } from "../../stores/workspace";
import RunDetail from "./RunDetail.vue";

// Master/detail (EPI-58): the sidebar owns selection; `:key` gives each run a
// fresh detail instance so per-run local state can't bleed (EPI-68).
const workspace = useWorkspace();
const runId = computed(() => workspace.selectedRunId);
</script>

<template>
  <RunDetail v-if="runId" :key="runId" :run-id="runId" />
  <div
    v-else
    class="h-full flex flex-col items-center justify-center text-center gap-2 text-(--ui-text-dimmed)"
  >
    <UIcon name="i-lucide-play" class="size-10" />
    <p class="text-sm">Select a run from the sidebar.</p>
    <p class="text-xs">Runs appear here when you classify a dataset.</p>
  </div>
</template>
