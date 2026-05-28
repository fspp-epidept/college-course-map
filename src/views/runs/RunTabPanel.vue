<script setup lang="ts">
import { computed } from "vue";
import { useRun } from "../../composables/useRuns";
import type { OpenTab } from "../../stores/workspace";

const props = defineProps<{ tab: OpenTab }>();

// Tab id shape: `run:<uuid>`.
const runId = computed(() => props.tab.id.replace(/^run:/, ""));

const { data: run, isPending, isError, error } = useRun(runId);

const progressPct = computed(() => {
  const r = run.value;
  if (!r?.rowsTotal || r.rowsProcessed === null || r.rowsProcessed === undefined) {
    return null;
  }
  return Math.round((r.rowsProcessed / r.rowsTotal) * 100);
});

function stateBadgeClass(state: string | undefined): string {
  switch (state) {
    case "running":
      return "bg-(--ui-color-info-500)/15 text-(--ui-color-info-500)";
    case "completed":
      return "bg-(--ui-color-success-500)/15 text-(--ui-color-success-500)";
    case "failed":
      return "bg-(--ui-color-error-500)/15 text-(--ui-color-error-500)";
    default:
      return "bg-(--ui-bg-muted) text-(--ui-text-muted)";
  }
}

function fmtTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  return new Date(iso).toLocaleString();
}
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-4 overflow-auto">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-play" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ tab.label }}</h2>
    </header>

    <p v-if="isPending" class="text-sm text-(--ui-text-dimmed)">Loading run…</p>
    <p v-else-if="isError" class="text-sm text-(--ui-color-error-500)">
      {{ error?.message }}
    </p>

    <template v-else-if="run">
      <div class="flex items-center gap-3">
        <span
          class="rounded-full px-2 py-0.5 text-xs uppercase tracking-wide"
          :class="stateBadgeClass(run.state)"
        >
          {{ run.state }}
        </span>
        <span v-if="run.digitLevel" class="text-sm text-(--ui-text-muted)">
          {{ run.digitLevel }}-digit model
        </span>
        <span v-if="run.executionProvider" class="text-sm text-(--ui-text-dimmed)">
          · {{ run.executionProvider }}
        </span>
      </div>

      <div v-if="progressPct !== null" class="flex flex-col gap-1.5">
        <div class="flex items-baseline justify-between text-sm">
          <span class="text-(--ui-text-muted)">
            {{ run.rowsProcessed }} / {{ run.rowsTotal }} rows
          </span>
          <span class="text-(--ui-text) tabular-nums">{{ progressPct }}%</span>
        </div>
        <UProgress
          :model-value="progressPct"
          :max="100"
          :color="run.state === 'failed' ? 'error' : 'primary'"
          size="sm"
        />
      </div>

      <div
        v-if="run.errorMessage"
        class="rounded-lg border border-(--ui-color-error-500)/40 bg-(--ui-color-error-500)/10 px-3 py-2 text-sm text-(--ui-color-error-500)"
      >
        {{ run.errorMessage }}
      </div>

      <dl class="grid grid-cols-2 gap-x-6 gap-y-2 text-sm">
        <dt class="text-(--ui-text-muted)">Dataset</dt>
        <dd class="text-(--ui-text)">{{ run.datasetTitle }}</dd>

        <dt class="text-(--ui-text-muted)">Description</dt>
        <dd class="text-(--ui-text)">{{ run.description ?? "—" }}</dd>

        <dt class="text-(--ui-text-muted)">Rows processed</dt>
        <dd class="text-(--ui-text) tabular-nums">{{ run.rowsProcessed ?? "—" }}</dd>

        <dt class="text-(--ui-text-muted)">New classifications</dt>
        <dd class="text-(--ui-text) tabular-nums">{{ run.uniqueInputsDone ?? "—" }}</dd>

        <dt class="text-(--ui-text-muted)">Cache hits</dt>
        <dd class="text-(--ui-text) tabular-nums">{{ run.cacheHits ?? "—" }}</dd>

        <dt class="text-(--ui-text-muted)">Created</dt>
        <dd class="text-(--ui-text)">{{ fmtTime(run.createdAt) }}</dd>

        <dt class="text-(--ui-text-muted)">Started</dt>
        <dd class="text-(--ui-text)">{{ fmtTime(run.startedAt) }}</dd>

        <dt class="text-(--ui-text-muted)">Completed</dt>
        <dd class="text-(--ui-text)">{{ fmtTime(run.completedAt) }}</dd>
      </dl>

      <p class="text-xs text-(--ui-text-dimmed)">Run id: <code>{{ run.id }}</code></p>
    </template>
  </div>
</template>
