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
  workspace.selectDataset(dataset.id);
}

// Prune a persisted selection whose dataset no longer exists in the DB (e.g.
// after a `task db:clear-data`). Without this the detail panel renders a
// stale dataset from localStorage and clicking Classify fails an FK
// constraint.
watch(
  datasets,
  (list) => {
    if (!list) return;
    const selected = workspace.selectedDatasetId;
    if (selected !== null && !list.some((d) => d.id === selected)) {
      workspace.selectDataset(null);
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
        class="text-left rounded px-2 py-1.5 flex flex-col"
        :class="
          workspace.selectedDatasetId === dataset.id
            ? 'bg-(--ui-bg-accented)'
            : 'hover:bg-(--ui-bg-muted)'
        "
        :aria-current="workspace.selectedDatasetId === dataset.id ? 'true' : undefined"
        @click="open(dataset)"
      >
        <span class="text-sm text-(--ui-text) flex items-center gap-1.5">
          <span class="truncate">{{ dataset.title }}</span>
          <span
            v-if="dataset.importState === 'importing'"
            class="text-(--ui-color-info-500) animate-pulse text-[10px] uppercase tracking-wide"
          >
            importing
          </span>
          <span
            v-else-if="dataset.importState === 'failed'"
            class="text-(--ui-color-error-500) text-[10px] uppercase tracking-wide"
          >
            failed
          </span>
        </span>
        <span class="text-xs text-(--ui-text-dimmed) tabular-nums">
          <template v-if="dataset.importState === 'importing'">
            {{ dataset.rowCount.toLocaleString() }} rows so far…
          </template>
          <template v-else>
            {{ dataset.rowCount.toLocaleString() }}
            {{ dataset.rowCount === 1 ? "course" : "courses" }}
            · {{ dataset.sourceKind }}
          </template>
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
