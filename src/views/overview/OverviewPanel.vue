<script setup lang="ts">
import { computed } from "vue";
import { useMetrics } from "../../composables/useMetrics";
import { useModelsEvents, useModelsStatus } from "../../composables/useModels";
import { useWorkspace } from "../../stores/workspace";

const { data: metrics, isPending, isError, error } = useMetrics();

// First-run callout (EPI-56): the connected build boots model-less until the
// user downloads from the Models panel. Events keep this banner live so it
// disappears on its own once models load.
const workspace = useWorkspace();
const { data: models } = useModelsStatus();
useModelsEvents();
const modelsReady = computed(() => models.value?.[0]?.loaded ?? true);
const modelsMissing = computed(
  () => models.value?.some((m) => m.filesPresent < m.filesTotal) ?? false,
);

function fmt(n: number | undefined): string {
  if (n === undefined) return "—";
  return n.toLocaleString();
}

const cacheHitLabel = computed(() => {
  const rate = metrics.value?.cacheHitRate;
  if (rate === null || rate === undefined) return "—";
  return `${Math.round(rate * 100)}%`;
});

const cards = computed(() => [
  {
    label: "Datasets",
    value: fmt(metrics.value?.datasets),
    hint: metrics.value?.datasets
      ? `${fmt(metrics.value.courses)} courses imported`
      : "Import a CSV to get started.",
  },
  {
    label: "Runs",
    value: fmt(metrics.value?.runs),
    hint: metrics.value?.runs
      ? `${fmt(metrics.value.completedRuns)} completed`
      : "No classification runs yet.",
  },
  {
    label: "Classifications",
    value: fmt(metrics.value?.classifications),
    hint: "Cached by (model, content hash).",
  },
  {
    label: "Cache hit rate",
    value: cacheHitLabel.value,
    hint: "Across all runs to date.",
  },
]);
</script>

<template>
  <div class="h-full p-8 flex flex-col gap-6 overflow-auto">
    <header>
      <h1 class="text-2xl font-semibold text-(--ui-text-highlighted)">
        College Course Map
      </h1>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Bulk-classify courses against CCM codes.
      </p>
    </header>

    <div
      v-if="!modelsReady"
      class="rounded-lg border border-(--ui-color-warning-500)/40 bg-(--ui-color-warning-500)/10 px-4 py-3 text-sm flex items-center justify-between gap-3"
    >
      <span class="text-(--ui-text)">
        <template v-if="modelsMissing">
          The classification models aren't installed yet (~1.8 GB download).
        </template>
        <template v-else>
          The classification models are still loading.
        </template>
      </span>
      <UButton
        v-if="modelsMissing"
        size="xs"
        color="warning"
        variant="soft"
        @click="workspace.setActiveActivity('models')"
      >
        Open Models
      </UButton>
    </div>

    <p v-if="isError" class="text-sm text-(--ui-color-error-500)">
      Failed to load metrics: {{ error?.message }}
    </p>

    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <div
        v-for="card in cards"
        :key="card.label"
        class="rounded-lg border border-(--ui-border) p-5 bg-(--ui-bg-elevated)"
      >
        <p class="text-xs uppercase tracking-wide text-(--ui-text-muted)">
          {{ card.label }}
        </p>
        <p class="mt-2 text-2xl font-medium tabular-nums">
          <span v-if="isPending" class="text-(--ui-text-dimmed)">…</span>
          <span v-else>{{ card.value }}</span>
        </p>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">{{ card.hint }}</p>
      </div>
    </div>
  </div>
</template>
