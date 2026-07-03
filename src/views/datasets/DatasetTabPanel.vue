<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { commands } from "../../bindings";
import { useCourses, useModelIdForDigitLevel } from "../../composables/useCourses";
import { useDatasets } from "../../composables/useDatasets";
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

// Note: we deliberately don't invalidate the courses query on every import
// progress tick. That cascade (listDatasets → invalidate courses →
// listCoursesWithResults) was hammering DuckDB while the Appender writer was
// busy and pinning the WebView main thread. The `useCourses` query is
// disabled outright while import_state === 'importing'; when it flips to
// 'ready' we invalidate once below.

const progressPct = computed(() => {
  const r = activeRun.value;
  if (!r?.rowsTotal || r.rowsProcessed === null || r.rowsProcessed === undefined) {
    return null;
  }
  return Math.round((r.rowsProcessed / r.rowsTotal) * 100);
});

const isRunning = computed(() => activeRun.value?.state === "running");

// Surface this dataset's import state by reusing the cached datasets query
// (it's already polled while any import is active). No extra IPC traffic.
const { data: datasets } = useDatasets();
const dataset = computed(() => datasets.value?.find((d) => d.id === datasetId.value));
const isImporting = computed(() => dataset.value?.importState === "importing");
const importFailed = computed(() => dataset.value?.importState === "failed");

// When import finishes (or fails), refresh the courses query exactly once so
// the table fills in. We watch the boolean transition rather than the row
// count so this fires at most twice per import (start + end), not per tick.
watch(isImporting, (now, before) => {
  if (before && !now) {
    queryClient.invalidateQueries({ queryKey: ["courses", datasetId.value] });
  }
});

// Classify never starts on the first click (EPI-66): the button opens a
// confirmation dialog stating scope and cache semantics, and only an explicit
// confirm starts the run. `digitLevel` (which drives the courses table's
// joined column) also only switches on confirm, so cancelling leaves the
// view untouched.
const confirmLevel = ref<2 | 4 | 6 | null>(null);
const confirmOpen = ref(false);

function requestRun(level: 2 | 4 | 6): void {
  confirmLevel.value = level;
  confirmOpen.value = true;
}

function confirmRun(): void {
  const level = confirmLevel.value;
  if (level === null) return;
  confirmOpen.value = false;
  digitLevel.value = level;
  classify.mutate(level);
}

// --- Courses table ---

const PAGE_SIZE = 50;
// Key-set pagination: `cursor` is the row_index of the first row in the
// current page (null = first page). `cursorStack` records the cursors of
// prior pages so Previous can pop back without recomputing — there's no
// way to derive "the page before cursor X" from a key-set scan, so we
// remember it explicitly.
const cursor = ref<number | null>(null);
const cursorStack = ref<number[]>([]);

// Reset pagination whenever the user switches digit level (the joined column
// changes underneath them).
watch(digitLevel, () => {
  cursor.value = null;
  cursorStack.value = [];
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
  cursor,
  pageSize: PAGE_SIZE,
  // Pause this query entirely while the import is still streaming rows. The
  // page is dynamic (cursor/limit) so each query is a real read against a
  // file that's getting hammered by the Appender; skipping while
  // importing keeps the UI responsive.
  enabled: computed(() => !isImporting.value),
});

const totalRows = computed(() => coursePage.value?.total ?? 0);
const pageRows = computed(() => coursePage.value?.rows ?? []);
// More pages exist iff we got a full page back — proxy for "the SQL would
// have returned more if we'd asked for it." Edge case: a dataset whose row
// count is an exact multiple of PAGE_SIZE will offer a Next click that lands
// on an empty page; Previous gets the user back. Worth living with for now.
const hasMore = computed(() => pageRows.value.length === PAGE_SIZE);
const hasPrev = computed(() => cursorStack.value.length > 0);

function gotoPrev(): void {
  const prev = cursorStack.value.pop();
  if (prev === undefined) return;
  // A popped value of 0 means "back to the first page" — represented as null
  // so the cursor IPC arg matches the initial state.
  cursor.value = prev === 0 ? null : prev;
}
function gotoNext(): void {
  const last = pageRows.value[pageRows.value.length - 1];
  if (!last) return;
  cursorStack.value.push(cursor.value ?? 0);
  cursor.value = last.rowIndex + 1;
}

// --- CSV export (EPI-15) ---
// Rust owns the save dialog and streams straight from DuckDB to disk; the
// resolved path comes back for display. `null` data means the user cancelled.

const exporting = ref(false);
const exportOutcome = ref<{ path: string; rows: number } | null>(null);
const exportError = ref<string | null>(null);

async function exportCsv(): Promise<void> {
  if (modelId.value == null) return;
  exporting.value = true;
  exportError.value = null;
  try {
    const result = await commands.exportResults({
      datasetId: datasetId.value,
      modelId: modelId.value,
    });
    if (result.status === "error") throw new Error(result.error);
    if (result.data) exportOutcome.value = result.data;
  } catch (e) {
    exportError.value = (e as Error).message;
  } finally {
    exporting.value = false;
  }
}
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-6 overflow-auto">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-database" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ tab.label }}</h2>
      <span
        v-if="isImporting"
        class="text-(--ui-color-info-500) animate-pulse text-xs uppercase tracking-wide"
      >
        importing
      </span>
      <span
        v-else-if="importFailed"
        class="text-(--ui-color-error-500) text-xs uppercase tracking-wide"
      >
        import failed
      </span>
    </header>

    <div
      v-if="isImporting"
      class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-1"
    >
      <span class="text-(--ui-text) font-medium">Importing rows in the background…</span>
      <span class="text-(--ui-text-muted) tabular-nums">
        {{ (dataset?.rowCount ?? 0).toLocaleString() }} rows so far
      </span>
      <span class="text-(--ui-text-dimmed) text-xs">
        Classify is disabled until the import finishes. The row count and
        the table below update every half second.
      </span>
    </div>

    <div
      v-else-if="importFailed && dataset?.importError"
      class="rounded-lg border border-(--ui-color-error-500)/40 bg-(--ui-color-error-500)/10 px-4 py-3 text-sm text-(--ui-color-error-500)"
    >
      Import failed: {{ dataset.importError }}
    </div>

    <section class="flex flex-col gap-3">
      <h3 class="text-sm font-medium text-(--ui-text)">Classify</h3>
      <div class="flex items-center gap-2">
        <UButton
          v-for="level in [2, 4, 6] as const"
          :key="level"
          :color="digitLevel === level ? 'primary' : 'neutral'"
          :variant="digitLevel === level ? 'solid' : 'outline'"
          :loading="classify.isPending.value && digitLevel === level"
          :disabled="classify.isPending.value || isRunning || isImporting || importFailed"
          @click="requestRun(level)"
        >
          Classify ({{ level }}-digit)
        </UButton>
      </div>

      <UModal
        v-model:open="confirmOpen"
        :title="`Start ${confirmLevel}-digit classification`"
        :ui="{ footer: 'justify-end' }"
      >
        <template #body>
          <div class="flex flex-col gap-2 text-sm text-(--ui-text-muted)">
            <p>
              Classifies the first 500 courses in
              <span class="text-(--ui-text)">{{ tab.label }}</span> with the
              {{ confirmLevel }}-digit CCM model.
            </p>
            <p>
              Runs locally on this machine. Courses already classified by this
              model are reused from the cache; only new course content is
              computed. Progress shows on this tab, and you can keep working
              while it runs.
            </p>
          </div>
        </template>
        <template #footer>
          <UButton variant="ghost" color="neutral" @click="confirmOpen = false">
            Cancel
          </UButton>
          <UButton color="primary" @click="confirmRun">Start run</UButton>
        </template>
      </UModal>

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

    <section v-if="!isImporting" class="flex flex-col gap-3 min-h-0">
      <div class="flex items-baseline justify-between gap-3">
        <h3 class="text-sm font-medium text-(--ui-text)">Courses</h3>
        <div class="flex items-baseline gap-3">
          <span class="text-xs text-(--ui-text-dimmed) tabular-nums">
            <template v-if="totalRows > 0">
              {{ pageRows.length.toLocaleString() }} of {{ totalRows.toLocaleString() }}
            </template>
          </span>
          <UButton
            variant="outline"
            color="neutral"
            icon="i-lucide-download"
            size="xs"
            :loading="exporting"
            :disabled="modelId == null || isRunning || totalRows === 0"
            @click="exportCsv"
          >
            Export CSV
          </UButton>
        </div>
      </div>

      <p v-if="exportError" class="text-sm text-(--ui-color-error-500)">
        Export failed: {{ exportError }}
      </p>
      <p v-else-if="exportOutcome" class="text-xs text-(--ui-text-muted)">
        Exported {{ exportOutcome.rows.toLocaleString() }} rows to
        <code>{{ exportOutcome.path }}</code>
      </p>

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
                <th class="px-3 py-2 text-right font-medium text-(--ui-text) w-24">
                  Confidence
                </th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-if="coursesPending && !coursePage"
                class="text-(--ui-text-dimmed)"
              >
                <td colspan="6" class="px-3 py-6 text-center">Loading courses…</td>
              </tr>
              <tr
                v-else-if="!coursePage || coursePage.rows.length === 0"
                class="text-(--ui-text-dimmed)"
              >
                <td colspan="6" class="px-3 py-6 text-center">
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
                  <UPopover v-if="row.classification">
                    <button
                      type="button"
                      class="font-mono text-(--ui-text) tabular-nums underline decoration-dotted underline-offset-2 cursor-pointer"
                    >
                      {{ row.classification }}
                    </button>
                    <template #content>
                      <div class="max-w-sm p-4 flex flex-col gap-2 text-sm">
                        <div class="flex items-baseline gap-2">
                          <code class="font-mono text-(--ui-text) tabular-nums">
                            {{ row.classification }}
                          </code>
                          <span
                            v-if="row.probability != null"
                            class="text-xs text-(--ui-text-muted) tabular-nums"
                          >
                            {{ (row.probability * 100).toFixed(1) }}% confidence
                          </span>
                        </div>
                        <template v-if="row.ccmTitle">
                          <p class="font-medium text-(--ui-text)">
                            {{ row.ccmTitle }}
                            <span
                              v-if="row.ccmTitleShort && row.ccmTitleShort !== row.ccmTitle"
                              class="font-normal text-(--ui-text-muted)"
                            >
                              ({{ row.ccmTitleShort }})
                            </span>
                          </p>
                          <p
                            v-if="row.ccmTitleLevel === 2 && digitLevel !== 2"
                            class="text-xs text-(--ui-text-dimmed)"
                          >
                            2-digit parent category — no official
                            {{ digitLevel }}-digit title exists for this code.
                          </p>
                          <p
                            v-if="row.ccmDescription"
                            class="text-(--ui-text-muted) leading-relaxed"
                          >
                            {{ row.ccmDescription }}
                          </p>
                        </template>
                        <p v-else class="text-(--ui-text-dimmed) text-xs">
                          No taxonomy entry for this code.
                        </p>
                      </div>
                    </template>
                  </UPopover>
                  <span v-else class="text-(--ui-text-dimmed)">—</span>
                </td>
                <td class="px-3 py-1.5 text-right tabular-nums">
                  <span v-if="row.probability != null" class="text-(--ui-text)">
                    {{ (row.probability * 100).toFixed(1) }}%
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
          :disabled="!hasPrev"
          @click="gotoPrev"
        >
          Previous
        </UButton>
        <UButton
          variant="ghost"
          color="neutral"
          trailing-icon="i-lucide-chevron-right"
          size="xs"
          :disabled="!hasMore"
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
