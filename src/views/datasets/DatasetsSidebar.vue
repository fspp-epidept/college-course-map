<script setup lang="ts">
import { ref, watch } from "vue";
import type { DatasetSummary } from "../../bindings";
import ImportCsvDialog from "../../components/ImportCsvDialog.vue";
import { useDatasets } from "../../composables/useDatasets";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();
const { data: datasets, isPending, isError, error } = useDatasets();

const importOpen = ref(false);

function open(dataset: DatasetSummary): void {
  workspace.openTab("datasets", {
    id: `dataset:${dataset.id}`,
    kind: "dataset",
    label: dataset.title,
    icon: "i-lucide-database",
  });
}

// Prune persisted tabs whose dataset no longer exists in the DB (e.g. after a
// `task db:clear-data`). Without this the workbench renders stale tabs from
// localStorage and clicking Classify on one fails an FK constraint.
watch(
  datasets,
  (list) => {
    if (!list) return;
    const valid = new Set(list.map((d) => `dataset:${d.id}`));
    const open = workspace.tabsByActivity.datasets ?? [];
    for (const tab of open) {
      if (!valid.has(tab.id)) {
        workspace.closeTab("datasets", tab.id);
      }
    }
  },
  { immediate: true },
);
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
      @click="importOpen = true"
    >
      <UIcon name="i-lucide-plus" class="size-4" />
      <span>Import CSV</span>
    </button>

    <ImportCsvDialog v-model:open="importOpen" />
  </div>
</template>
