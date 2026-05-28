<script setup lang="ts">
import { computed } from "vue";
import { useMetrics } from "../../composables/useMetrics";

const { data: metrics, isPending, isError, error } = useMetrics();

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
