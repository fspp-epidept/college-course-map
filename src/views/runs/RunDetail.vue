<script setup lang="ts">
import { computed, ref } from "vue";
import {
  resumeBlockerText,
  useDeleteRun,
  usePauseRun,
  useResumeRun,
  useRun,
  useRunRate,
} from "../../composables/useRuns";
import { useWorkspace } from "../../stores/workspace";
import { runStateMeta } from "./runState";

// Master/detail (EPI-58): RunsPanel keys this component by run id.
const props = defineProps<{ runId: string }>();

const currentRunId = computed(() => props.runId);

const { data: run, isPending, isError, error } = useRun(currentRunId);

// Heading: dataset title + a short id suffix (same label the sidebar rows
// and the old tab strip used).
const heading = computed(() =>
  run.value ? `${run.value.datasetTitle} · ${run.value.id.slice(0, 8)}` : "Run",
);

const pauseRun = usePauseRun();
function onPause() {
  pauseRun.mutate(currentRunId.value);
}

const resumeRun = useResumeRun();
function onResume() {
  resumeRun.mutate(currentRunId.value);
}

// Delete (#198): the record only. Clear the selection first so this
// component unmounts instead of refetching a run that no longer exists.
const workspace = useWorkspace();
const deleteRun = useDeleteRun();
const deleteOpen = ref(false);
function onDelete() {
  deleteRun.mutate(currentRunId.value, {
    onSuccess: () => {
      deleteOpen.value = false;
      workspace.selectRun(null);
    },
  });
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
      <h2 class="text-lg font-medium">{{ heading }}</h2>
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
        <div class="ml-auto flex items-center gap-2">
          <UButton
            v-if="run.state === 'running'"
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
            color="primary"
            size="xs"
            icon="i-lucide-play"
            :loading="resumeRun.isPending.value"
            @click="onResume"
          >
            Resume
          </UButton>
          <UButton
            v-if="run.state !== 'running'"
            color="error"
            variant="outline"
            size="xs"
            icon="i-lucide-trash-2"
            @click="deleteOpen = true"
          >
            Delete Run…
          </UButton>
        </div>
      </div>

      <UModal v-model:open="deleteOpen" title="Delete this run?">
        <template #body>
          <div class="flex flex-col gap-3 text-sm">
            <p class="text-(--ui-text-muted)">
              This removes the run's record from the list. The classifications it
              computed are kept and reused by the next run on the same courses.
            </p>
            <p v-if="deleteRun.error.value" class="text-(--ui-color-error-500)">
              Delete failed: {{ deleteRun.error.value.message }}
            </p>
          </div>
        </template>
        <template #footer>
          <div class="flex justify-end gap-2 w-full">
            <UButton variant="ghost" color="neutral" @click="deleteOpen = false">Cancel</UButton>
            <UButton color="error" :loading="deleteRun.isPending.value" @click="onDelete">
              Delete Run
            </UButton>
          </div>
        </template>
      </UModal>

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
