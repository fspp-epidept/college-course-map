<script setup lang="ts">
import { computed } from "vue";
import {
  resumeBlockerText,
  usePauseRun,
  useResumeRun,
  useRun,
  useRunRate,
} from "../../composables/useRuns";
import { runStateMeta } from "./runState";
import type { OpenTab } from "../../stores/workspace";

const props = defineProps<{ tab: OpenTab }>();

// Tab id shape: `run:<uuid>`.
const runId = computed(() => props.tab.id.replace(/^run:/, ""));

const { data: run, isPending, isError, error } = useRun(runId);

const pauseRun = usePauseRun();
function onPause() {
  pauseRun.mutate(runId.value);
}

const resumeRun = useResumeRun();
function onResume() {
  resumeRun.mutate(runId.value);
}

const rate = useRunRate(run);
const rateLabel = computed(() =>
  rate.value === null ? null : `≈ ${Math.round(rate.value).toLocaleString()} classifications/s`,
);

const progressPct = computed(() => {
  const r = run.value;
  if (!r?.rowsTotal || r.rowsProcessed === null || r.rowsProcessed === undefined) {
    return null;
  }
  return Math.round((r.rowsProcessed / r.rowsTotal) * 100);
});

// Dataset rows behind the row×model progress units (EPI-96): the counters
// sum across models, so a 999,999-row dataset legitimately totals ~3M
// classifications — say so instead of calling them "rows".
const rowsBreakdown = computed(() => {
  const r = run.value;
  if (!r?.rowsTotal || r.modelCount <= 1) return null;
  return `${Math.round(r.rowsTotal / r.modelCount).toLocaleString()} rows × ${r.modelCount} models`;
});

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
          class="rounded-full px-2 py-0.5 text-xs uppercase tracking-wide inline-flex items-center gap-1"
          :class="runStateMeta(run.state).badgeClass"
        >
          <UIcon :name="runStateMeta(run.state).icon" class="size-3" />
          {{ runStateMeta(run.state).label }}
        </span>
        <span class="text-sm text-(--ui-text-muted)">
          {{ run.digitLevel ? `${run.digitLevel}-digit model` : "all models" }}
        </span>
        <span v-if="run.executionProvider" class="text-sm text-(--ui-text-dimmed)">
          · {{ run.executionProvider }}
        </span>
        <UButton
          v-if="run.state === 'running'"
          class="ml-auto"
          color="neutral"
          variant="subtle"
          size="xs"
          icon="i-lucide-pause"
          :loading="pauseRun.isPending.value"
          @click="onPause"
        >
          Pause
        </UButton>
        <UButton
          v-else-if="run.resumable"
          class="ml-auto"
          color="primary"
          size="xs"
          icon="i-lucide-play"
          :loading="resumeRun.isPending.value"
          @click="onResume"
        >
          Resume
        </UButton>
      </div>

      <div
        v-if="run.state === 'interrupted' && !run.resumable"
        class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 text-xs text-(--ui-text-muted) flex flex-col gap-1"
      >
        <span class="text-(--ui-text) font-medium">This run can't resume right now</span>
        <span v-for="blocker in run.resumeBlockers" :key="blocker">
          {{ resumeBlockerText(blocker) }}
        </span>
      </div>

      <p v-if="resumeRun.isError.value" class="text-sm text-(--ui-color-error-500)">
        Resume failed: {{ resumeRun.error.value?.message }}
      </p>

      <div v-if="progressPct !== null" class="flex flex-col gap-1.5">
        <div class="flex items-baseline justify-between text-sm">
          <span class="text-(--ui-text-muted)">
            {{ run.rowsProcessed?.toLocaleString() }} /
            {{ run.rowsTotal?.toLocaleString() }} classifications
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

        <dt class="text-(--ui-text-muted)">Classifications</dt>
        <dd class="text-(--ui-text) tabular-nums">
          {{ run.rowsProcessed?.toLocaleString() ?? "—" }}
          <span v-if="rowsBreakdown" class="text-(--ui-text-dimmed)">
            ({{ rowsBreakdown }})
          </span>
        </dd>

        <template v-if="rateLabel">
          <dt class="text-(--ui-text-muted)">Throughput</dt>
          <dd class="text-(--ui-text) tabular-nums">{{ rateLabel }}</dd>
        </template>

        <dt class="text-(--ui-text-muted)">New classifications</dt>
        <dd class="text-(--ui-text) tabular-nums">
          {{ run.uniqueInputsDone?.toLocaleString() ?? "—" }}
        </dd>

        <dt class="text-(--ui-text-muted)">Cache hits</dt>
        <dd class="text-(--ui-text) tabular-nums">
          {{ run.cacheHits?.toLocaleString() ?? "—" }}
        </dd>

        <template v-if="run.resumeCount > 0">
          <dt class="text-(--ui-text-muted)">Resumed</dt>
          <dd class="text-(--ui-text) tabular-nums">
            {{ run.resumeCount }} {{ run.resumeCount === 1 ? "time" : "times" }}
          </dd>
        </template>

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
