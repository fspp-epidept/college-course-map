<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { commands } from "../../bindings";
import { useCourses, useModelIdForDigitLevel } from "../../composables/useCourses";
import { useRun } from "../../composables/useRuns";
import type { OpenTab } from "../../stores/workspace";

const props = defineProps<{ tab: OpenTab }>();

// Tabs in this activity use ids shaped like `dataset:<uuid>`. Strip the prefix
// to get the dataset id this tab is bound to.
const datasetId = computed(() => props.tab.id.replace(/^dataset:/, ""));

const digitLevel = ref<2 | 4 | 6>(6);
// Run id of the most-recently-started run on this dataset tab. The live
// progress meter polls this id; when null nothing renders.
const activeRunId = ref<string | null>(null);
const startError = ref<string | null>(null);

const queryClient = useQueryClient();

const classify = useMutation({
  mutationFn: async (level: 2 | 4 | 6) => {
    const result = await commands.startRun({
      datasetId: datasetId.value,
      digitLevel: level,
      limit: 500,
    });
    if (result.status === "error") throw new Error(result.error);
    return { ...result.data, digitLevel: level };
  },
  onSuccess: (data) => {
    activeRunId.value = data.runId;
    startError.value = null;
    queryClient.invalidateQueries({ queryKey: ["datasets"] });
    queryClient.invalidateQueries({ queryKey: ["metrics"] });
    queryClient.invalidateQueries({ queryKey: ["runs"] });
  },
  onError: (err: Error) => {
    startError.value = err.message;
  },
});

// `useRun` polls every 250 ms while state === 'running'. When the run we
// kicked off completes, also nudge the global query caches so the sidebar /
// metrics surfaces refresh exactly once and the courses table pulls fresh
// classification columns.
const runQueryId = computed(() => activeRunId.value ?? "");
const { data: activeRun } = useRun(runQueryId);

watch(
  () => activeRun.value?.state,
  (state) => {
    if (state === "completed" || state === "failed") {
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
      queryClient.invalidateQueries({ queryKey: ["runs"] });
      queryClient.invalidateQueries({ queryKey: ["courses"] });
    }
  },
);

// While a run is running, keep the courses table fresh so newly-written
// classifications surface every poll cycle.
watch(
  () => activeRun.value?.rowsProcessed,
  (next, prev) => {
    if (activeRun.value?.state !== "running") return;
    if (next === prev) return;
    queryClient.invalidateQueries({ queryKey: ["courses", datasetId.value] });
  },
);

const progressPct = computed(() => {
  const r = activeRun.value;
  if (!r?.rowsTotal || r.rowsProcessed === null || r.rowsProcessed === undefined) {
    return null;
  }
  return Math.round((r.rowsProcessed / r.rowsTotal) * 100);
});

const isRunning = computed(() => activeRun.value?.state === "running");

function runFor(level: 2 | 4 | 6): void {
  digitLevel.value = level;
  classify.mutate(level);
}

// --- Courses table ---

const PAGE_SIZE = 50;
const page = ref(0);

// Reset to page 0 whenever the user switches digit level (the joined column
// changes underneath them).
watch(digitLevel, () => {
  page.value = 0;
});

const { data: modelId } = useModelIdForDigitLevel(digitLevel);
const {
  data: coursePage,
  isPending: coursesPending,
  isError: coursesError,
  error: coursesErr,
} = useCourses({
  datasetId,
  modelId: computed(() => modelId.value ?? null),
  page,
  pageSize: PAGE_SIZE,
});

const totalRows = computed(() => coursePage.value?.total ?? 0);
const totalPages = computed(() =>
  totalRows.value === 0 ? 0 : Math.ceil(totalRows.value / PAGE_SIZE),
);
const pageStart = computed(() => (totalRows.value === 0 ? 0 : page.value * PAGE_SIZE + 1));
const pageEnd = computed(() => Math.min((page.value + 1) * PAGE_SIZE, totalRows.value));

function gotoPrev(): void {
  if (page.value > 0) page.value -= 1;
}
function gotoNext(): void {
  if (page.value + 1 < totalPages.value) page.value += 1;
}
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-6 overflow-auto">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-database" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ tab.label }}</h2>
    </header>

    <section class="flex flex-col gap-3">
      <h3 class="text-sm font-medium text-(--ui-text)">Classify</h3>
      <p class="text-sm text-(--ui-text-muted)">
        Run real Rust ONNX inference against the first 500 courses in this dataset.
        Results are cached by (model, content hash) — re-running the same digit
        level will mostly hit the cache instead of recomputing.
      </p>
      <div class="flex items-center gap-2">
        <UButton
          v-for="level in [2, 4, 6] as const"
          :key="level"
          :color="digitLevel === level ? 'primary' : 'neutral'"
          :variant="digitLevel === level ? 'solid' : 'outline'"
          :loading="classify.isPending.value && digitLevel === level"
          :disabled="classify.isPending.value || isRunning"
          @click="runFor(level)"
        >
          Classify ({{ level }}-digit)
        </UButton>
      </div>

      <div
        v-if="startError"
        class="rounded-lg border border-(--ui-color-error-500)/40 bg-(--ui-color-error-500)/10 px-3 py-2 text-sm text-(--ui-color-error-500)"
      >
        {{ startError }}
      </div>

      <div
        v-if="activeRun"
        class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-2"
      >
        <div class="flex items-center justify-between">
          <span class="text-(--ui-text) font-medium">
            <template v-if="activeRun.state === 'running'">
              Classifying ({{ activeRun.digitLevel }}-digit)…
            </template>
            <template v-else-if="activeRun.state === 'completed'">
              {{ activeRun.digitLevel }}-digit run complete
            </template>
            <template v-else-if="activeRun.state === 'failed'">
              {{ activeRun.digitLevel }}-digit run failed
            </template>
            <template v-else>
              {{ activeRun.state }}
            </template>
          </span>
          <span v-if="progressPct !== null" class="tabular-nums text-(--ui-text-muted)">
            {{ progressPct }}%
          </span>
        </div>

        <UProgress
          v-if="progressPct !== null"
          :model-value="progressPct"
          :max="100"
          :color="activeRun.state === 'failed' ? 'error' : 'primary'"
          size="sm"
        />

        <div class="grid grid-cols-2 gap-x-6 gap-y-1 text-(--ui-text-muted)">
          <span>Rows processed</span>
          <span class="text-(--ui-text) tabular-nums">
            {{ activeRun.rowsProcessed ?? 0 }} / {{ activeRun.rowsTotal ?? 0 }}
          </span>
          <span>New classifications</span>
          <span class="text-(--ui-text) tabular-nums">{{ activeRun.uniqueInputsDone ?? 0 }}</span>
          <span>Cache hits</span>
          <span class="text-(--ui-text) tabular-nums">{{ activeRun.cacheHits ?? 0 }}</span>
        </div>

        <p
          v-if="activeRun.errorMessage"
          class="text-(--ui-color-error-500) text-xs"
        >
          {{ activeRun.errorMessage }}
        </p>
      </div>
    </section>

    <section class="flex flex-col gap-3 min-h-0">
      <div class="flex items-baseline justify-between gap-3">
        <h3 class="text-sm font-medium text-(--ui-text)">Courses</h3>
        <span class="text-xs text-(--ui-text-dimmed) tabular-nums">
          <template v-if="totalRows > 0">
            {{ pageStart.toLocaleString() }}–{{ pageEnd.toLocaleString() }}
            of {{ totalRows.toLocaleString() }}
          </template>
        </span>
      </div>

      <p v-if="coursesError" class="text-sm text-(--ui-color-error-500)">
        Failed to load courses: {{ coursesErr?.message }}
      </p>

      <div class="rounded-lg border border-(--ui-border) overflow-hidden">
        <div class="overflow-x-auto">
          <table class="min-w-full text-xs">
            <thead class="bg-(--ui-bg-muted)">
              <tr>
                <th class="px-3 py-2 text-left font-medium text-(--ui-text) w-14">#</th>
                <th class="px-3 py-2 text-left font-medium text-(--ui-text)">Subject</th>
                <th class="px-3 py-2 text-left font-medium text-(--ui-text)">Catalog</th>
                <th class="px-3 py-2 text-left font-medium text-(--ui-text)">Title</th>
                <th class="px-3 py-2 text-left font-medium text-(--ui-text)">
                  {{ digitLevel }}-digit CCM
                </th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-if="coursesPending && !coursePage"
                class="text-(--ui-text-dimmed)"
              >
                <td colspan="5" class="px-3 py-6 text-center">Loading courses…</td>
              </tr>
              <tr
                v-else-if="!coursePage || coursePage.rows.length === 0"
                class="text-(--ui-text-dimmed)"
              >
                <td colspan="5" class="px-3 py-6 text-center">
                  No courses in this dataset.
                </td>
              </tr>
              <tr
                v-for="row in coursePage?.rows"
                v-else
                :key="row.id"
                class="border-t border-(--ui-border-muted)"
              >
                <td class="px-3 py-1.5 text-(--ui-text-dimmed) tabular-nums">{{ row.rowIndex }}</td>
                <td class="px-3 py-1.5 text-(--ui-text)">{{ row.subjectCode ?? "—" }}</td>
                <td class="px-3 py-1.5 text-(--ui-text)">{{ row.catalogNumber ?? "—" }}</td>
                <td class="px-3 py-1.5 text-(--ui-text)">{{ row.courseTitle ?? "—" }}</td>
                <td class="px-3 py-1.5">
                  <span
                    v-if="row.classification"
                    class="font-mono text-(--ui-text) tabular-nums"
                  >
                    {{ row.classification }}
                  </span>
                  <span v-else class="text-(--ui-text-dimmed)">—</span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>

      <div class="flex items-center justify-end gap-2">
        <UButton
          variant="ghost"
          color="neutral"
          icon="i-lucide-chevron-left"
          size="xs"
          :disabled="page === 0"
          @click="gotoPrev"
        >
          Previous
        </UButton>
        <span class="text-xs text-(--ui-text-muted) tabular-nums px-2">
          Page {{ page + 1 }} of {{ Math.max(totalPages, 1) }}
        </span>
        <UButton
          variant="ghost"
          color="neutral"
          trailing-icon="i-lucide-chevron-right"
          size="xs"
          :disabled="page + 1 >= totalPages"
          @click="gotoNext"
        >
          Next
        </UButton>
      </div>
    </section>

    <p class="text-xs text-(--ui-text-dimmed)">
      Dataset id: <code>{{ datasetId }}</code>
    </p>
  </div>
</template>
