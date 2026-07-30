<script setup lang="ts">
import { computed } from "vue";
import { useWorkspace } from "../../stores/workspace";
import DatasetDetail from "./DatasetDetail.vue";

// Master/detail (EPI-58): the sidebar owns selection; this panel renders the
// selected dataset's detail or the empty state. `:key` forces a fresh detail
// instance per dataset — without it Vue reuses one instance and
// component-local state (pagination cursors, view level, error banners)
// silently bleeds from one dataset to the next (EPI-68).
const workspace = useWorkspace();
const datasetId = computed(() => workspace.selectedDatasetId);
</script>

<template>
  <DatasetDetail v-if="datasetId" :key="datasetId" :dataset-id="datasetId" />
  <div
    v-else
    class="h-full flex flex-col items-center justify-center text-center gap-2 text-(--ui-text-dimmed)"
  >
    <UIcon name="i-lucide-database" class="size-10" />
    <p class="text-sm">Select a dataset from the sidebar.</p>
    <p class="text-xs">Or import a CSV to get started.</p>
  </div>
</template>
