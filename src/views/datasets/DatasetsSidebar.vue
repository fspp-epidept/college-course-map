<script setup lang="ts">
import type { DatasetSummary } from "../../bindings";
import { useDatasets } from "../../composables/useDatasets";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();
const { data: datasets, isPending, isError, error } = useDatasets();

function open(dataset: DatasetSummary): void {
  workspace.openTab("datasets", {
    id: `dataset:${dataset.id}`,
    kind: "dataset",
    label: dataset.title,
    icon: "i-lucide-database",
  });
}
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <p v-if="isPending" class="px-2 py-1.5 text-sm text-(--ui-text-dimmed)">
      Loading datasets…
    </p>

    <p v-else-if="isError" class="px-2 py-1.5 text-sm text-(--ui-color-error-500)">
      Failed to load datasets: {{ error?.message }}
    </p>

    <template v-else-if="datasets && datasets.length > 0">
      <button
        v-for="dataset in datasets"
        :key="dataset.id"
        type="button"
        class="text-left rounded px-2 py-1.5 hover:bg-(--ui-bg-muted) flex flex-col"
        @click="open(dataset)"
      >
        <span class="text-sm text-(--ui-text)">{{ dataset.title }}</span>
        <span class="text-xs text-(--ui-text-dimmed)">
          {{ dataset.rowCount }} {{ dataset.rowCount === 1 ? "course" : "courses" }}
          · {{ dataset.sourceKind }}
        </span>
      </button>
    </template>

    <p v-else class="px-2 py-1.5 text-sm text-(--ui-text-dimmed)">
      No datasets yet. Import a CSV to get started.
    </p>

    <button
      type="button"
      class="mt-2 text-left rounded px-2 py-1.5 text-sm text-(--ui-text-muted) hover:bg-(--ui-bg-muted) flex items-center gap-2"
    >
      <UIcon name="i-lucide-plus" class="size-4" />
      <span>Import CSV</span>
    </button>
  </div>
</template>
