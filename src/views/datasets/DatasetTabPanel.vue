<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { commands } from "../../bindings";
import { useCourses, useCoverage, useModelIdForDigitLevel } from "../../composables/useCourses";
import { useDatasets } from "../../composables/useDatasets";
import {
  resumeBlockerText,
  useLatestRun,
  usePauseRun,
  useResumeRun,
  useRuns,
} from "../../composables/useRuns";
import type { OpenTab } from "../../stores/workspace";

const props = defineProps<{ tab: OpenTab }>();

// Tabs in this activity use ids shaped like `dataset:<uuid>`. Strip the prefix
// to get the dataset id this tab is bound to.
const datasetId = computed(() => props.tab.id.replace(/^dataset:/, ""));

type DigitLevel = 2 | 4 | 6;
const LEVELS: readonly DigitLevel[] = [2, 4, 6] as const;

// Which model's results the courses table shows. Pure view state — switching
// never starts work (EPI-68 view/action split). Fresh per tab: TabbedView
// keys the component by tab id.
const viewLevel = ref<DigitLevel>(6);

const queryClient = useQueryClient();

// --- Run state (backend-derived, EPI-68) ---
// The run surface card renders from "the dataset's latest run" as the backend
// reports it — not from a component-local run id — so it survives tab
// close/reopen and app restart. Polls 250 ms while running.
const { data: latestRun } = useLatestRun(datasetId);
const isRunning = computed(() => latestRun.value?.state === "running");

// Global runs list (1 s heartbeat while anything runs) tells this tab about
// runs on *other* datasets: only one run may be active app-wide.
const { data: allRuns } = useRuns();
const activeElsewhere = computed(() => {
  const other = allRuns.value?.find(
    (r) => r.state === "running" && r.datasetId !== datasetId.value,
  );
  return other ?? null;
});

// --- Coverage (EPI-68) ---
// Per-model classified/total counts: feeds the view-switcher chips and the
// confirm panel's "already classified" line.
const { data: coverage, refetch: refetchCoverage } = useCoverage(datasetId);
function coverageFor(level: DigitLevel) {
  return coverage.value?.find((c) => c.digitLevel === level) ?? null;
}
function coverageLabel(level: DigitLevel): string {
  const c = coverageFor(level);
  if (!c || c.total === 0 || c.classified === 0) return "—";
  const pct = Math.floor((c.classified / c.total) * 100);
  // A dataset that's classified-but-not-quite-100% floors to 99, never
  // rounds up to a dishonest 100.
  return `${c.classified >= c.total ? 100 : Math.min(pct, 99)}%`;
}

// --- Classify action ---
// A run always covers every model (EPI-96) — one button, one confirm.

const confirmOpen = ref(false);
const startError = ref<string | null>(null);

const classify = useMutation({
  mutationFn: async () => {
    const result = await commands.startRun({ datasetId: datasetId.value });
    if (result.status === "error") throw new Error(result.error);
    return result.data;
  },
  onSuccess: () => {
    confirmOpen.value = false;
    startError.value = null;
    // ["runs"] prefix-matches the latest-run query, which flips the card to
    // its running state on the next render.
    queryClient.invalidateQueries({ queryKey: ["runs"] });
    queryClient.invalidateQueries({ queryKey: ["datasets"] });
    queryClient.invalidateQueries({ queryKey: ["metrics"] });
  },
  onError: (err: Error) => {
    startError.value = err.message;
  },
});

function requestRun(): void {
  startError.value = null;
  confirmOpen.value = true;
  // The confirm panel quotes cache numbers; make sure they're current at the
  // moment of decision, not from tab-mount time.
  refetchCoverage();
}

// Confirm-panel numbers: what each level still needs to compute.
const confirmLevels = computed(() =>
  LEVELS.map((level) => {
    const c = coverageFor(level);
    return {
      level,
      remaining: c ? c.total - c.classified : null,
      classified: c?.classified ?? 0,
    };
  }),
);

// --- Pause ---

const pauseRun = usePauseRun();
// `pause_run` flips a flag; the worker still drains its current batch before
// the run flips to `interrupted`. Track the request locally so the card can
// say "Pausing…" during that honest gap.
const pauseRequested = ref(false);
function onPause(): void {
  if (!latestRun.value) return;
  pauseRequested.value = true;
  pauseRun.mutate(latestRun.value.id);
}
watch(isRunning, (running) => {
  if (!running) pauseRequested.value = false;
});

// --- Resume (EPI-38) ---

const resumeRun = useResumeRun();
function onResume(): void {
  if (!latestRun.value) return;
  resumeRun.mutate(latestRun.value.id);
}

// --- Dataset import state ---
// Surface this dataset's import state by reusing the cached datasets query
// (it's already polled while any import is active). No extra IPC traffic.
const { data: datasets } = useDatasets();
const dataset = computed(() => datasets.value?.find((d) => d.id === datasetId.value));
const isImporting = computed(() => dataset.value?.importState === "importing");
const importFailed = computed(() => dataset.value?.importState === "failed");

// When import finishes (or fails), refresh the courses + coverage queries
// exactly once so the table fills in.
watch(isImporting, (now, before) => {
  if (before && !now) {
    queryClient.invalidateQueries({ queryKey: ["courses", datasetId.value] });
    queryClient.invalidateQueries({ queryKey: ["coverage", datasetId.value] });
  }
});

const classifyDisabled = computed(
  () =>
    classify.isPending.value ||
    isRunning.value ||
    activeElsewhere.value !== null ||
    isImporting.value ||
    importFailed.value,
);

// --- Run card presentation ---

const progressPct = computed(() => {
  const r = latestRun.value;
  if (!r?.rowsTotal || r.rowsProcessed === null || r.rowsProcessed === undefined) {
    return null;
  }
  return Math.round((r.rowsProcessed / r.rowsTotal) * 100);
});

const runStateHeading = computed(() => {
  const r = latestRun.value;
  if (!r) return "";
  const level = r.digitLevel ? `${r.digitLevel}-digit` : "All-models";
  if (r.state === "running" && pauseRequested.value)
    return "Pausing — finishing the current batch…";
  switch (r.state) {
    case "running":
      return `Classifying (${level.toLowerCase()})…`;
    case "completed":
      return `${level} run complete`;
    case "interrupted":
      return `${level} run paused`;
    case "failed":
      return `${level} run failed`;
    default:
      return `${level} run ${r.state}`;
  }
});

// While a run is writing results for the level being viewed, keep the visible
// table page fresh. Other levels' columns can't change, so don't refetch them.
watch(
  () => latestRun.value?.rowsProcessed,
  (next, prev) => {
    if (!isRunning.value || next === prev) return;
    if (latestRun.value?.digitLevel === viewLevel.value) {
      queryClient.invalidateQueries({ queryKey: ["courses", datasetId.value] });
    }
  },
);

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
watch(viewLevel, () => {
  cursor.value = null;
  cursorStack.value = [];
});

const { data: modelId } = useModelIdForDigitLevel(viewLevel);
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

// --- CSV export (EPI-15, EPI-77/79/81/98) ---
// The Export button opens a small options dialog; confirming closes it and
// hands off to Rust, which owns the save dialog and streams straight from
// DuckDB to disk. The resolved path comes back for display. `null` data
// means the user cancelled the save dialog.

const exporting = ref(false);
const exportOutcome = ref<{ path: string; rows: number } | null>(null);
const exportError = ref<string | null>(null);
const exportOpen = ref(false);
// Top-5 candidate columns are an explicit opt-in (EPI-98 stakeholder
// decision): 15 extra columns is a lot of CSV to include silently.
const includeTopCandidates = ref(false);

async function exportCsv(): Promise<void> {
  if (modelId.value == null) return;
  exportOpen.value = false;
  exporting.value = true;
  exportError.value = null;
  try {
    const result = await commands.exportResults({
      datasetId: datasetId.value,
      modelId: modelId.value,
      includeTopCandidates: includeTopCandidates.value,
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
      <!-- Classify action: one button, one confirm (EPI-96 — a run always
           covers every model). Selection of what to LOOK at lives in the
           table header below; this control only starts work. -->
      <div class="flex items-center gap-3">
        <UButton
          color="primary"
          icon="i-lucide-play"
          :disabled="classifyDisabled"
          @click="requestRun"
        >
          Classify
        </UButton>
        <span v-if="activeElsewhere" class="text-xs text-(--ui-text-muted)">
          A run is active on
          <span class="text-(--ui-text)">{{ activeElsewhere.datasetTitle }}</span>
          — pause it or wait for it to finish.
        </span>
      </div>

      <!-- Inline confirm panel (EPI-66's confirm requirement, EPI-68's form).
           Expands in place of the run card; the numbers sit next to the table
           they describe. No overlay, nothing modal to dismiss. -->
      <Transition
        mode="out-in"
        enter-active-class="transition duration-200 ease-out motion-reduce:transition-none"
        enter-from-class="opacity-0 -translate-y-1"
        leave-active-class="transition duration-150 ease-out motion-reduce:transition-none"
        leave-to-class="opacity-0 -translate-y-1"
      >
        <div
          v-if="confirmOpen"
          key="confirm"
          class="rounded-lg border border-(--ui-border-accented) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-2"
        >
          <span class="text-(--ui-text) font-medium">
            Classify {{ tab.label }} with all models
          </span>
          <ul class="text-(--ui-text-muted) flex flex-col gap-0.5">
            <li v-for="row in confirmLevels" :key="row.level" class="tabular-nums">
              {{ row.level }}-digit:
              <template v-if="row.remaining !== null">
                <span class="text-(--ui-text)">{{ row.remaining.toLocaleString() }}</span>
                to compute<template v-if="row.classified > 0"
                  >,
                  <span class="text-(--ui-text)">{{ row.classified.toLocaleString() }}</span>
                  reused from the cache</template
                >
              </template>
              <template v-else>
                all
                <span class="text-(--ui-text)">
                  {{ (dataset?.rowCount ?? totalRows).toLocaleString() }}
                </span>
                courses
              </template>
            </li>
          </ul>
          <p class="text-(--ui-text-dimmed) text-xs">
            Runs locally on this machine, one model at a time. You can keep
            working while it runs.
          </p>
          <p v-if="startError" class="text-(--ui-color-error-500) text-xs">
            {{ startError }}
          </p>
          <div class="flex justify-end gap-2 pt-1">
            <UButton variant="ghost" color="neutral" @click="confirmOpen = false">
              Cancel
            </UButton>
            <UButton
              color="primary"
              :loading="classify.isPending.value"
              :disabled="classifyDisabled"
              @click="classify.mutate()"
            >
              Start run
            </UButton>
          </div>
        </div>

        <!-- Run surface card: the dataset's latest run as the backend reports
             it. Confirm panel takes its place while a decision is pending. -->
        <div
          v-else-if="latestRun"
          key="run-card"
          class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-2"
        >
          <div class="flex items-center justify-between gap-3">
            <span class="text-(--ui-text) font-medium">{{ runStateHeading }}</span>
            <div class="flex items-center gap-3">
              <span v-if="progressPct !== null && isRunning" class="tabular-nums text-(--ui-text-muted)">
                {{ progressPct }}%
              </span>
              <UButton
                v-if="isRunning"
                color="neutral"
                variant="subtle"
                size="xs"
                icon="i-lucide-pause"
                :disabled="pauseRequested"
                @click="onPause"
              >
                {{ pauseRequested ? "Pausing…" : "Pause" }}
              </UButton>
              <UButton
                v-else-if="latestRun.resumable"
                color="primary"
                size="xs"
                icon="i-lucide-play"
                :loading="resumeRun.isPending.value"
                :disabled="activeElsewhere !== null"
                @click="onResume"
              >
                Resume
              </UButton>
            </div>
          </div>

          <UProgress
            v-if="progressPct !== null && isRunning"
            :model-value="progressPct"
            :max="100"
            color="primary"
            size="sm"
          />

          <div class="grid grid-cols-2 gap-x-6 gap-y-1 text-(--ui-text-muted)">
            <span>Classifications</span>
            <span class="text-(--ui-text) tabular-nums">
              {{ (latestRun.rowsProcessed ?? 0).toLocaleString() }} /
              {{ (latestRun.rowsTotal ?? 0).toLocaleString() }}
              <span
                v-if="latestRun.modelCount > 1 && latestRun.rowsTotal"
                class="text-(--ui-text-dimmed)"
              >
                ({{ Math.round(latestRun.rowsTotal / latestRun.modelCount).toLocaleString() }}
                rows × {{ latestRun.modelCount }} models)
              </span>
            </span>
            <span>New classifications</span>
            <span class="text-(--ui-text) tabular-nums">
              {{ (latestRun.uniqueInputsDone ?? 0).toLocaleString() }}
            </span>
            <span>Cache hits</span>
            <span class="text-(--ui-text) tabular-nums">
              {{ (latestRun.cacheHits ?? 0).toLocaleString() }}
            </span>
          </div>

          <p
            v-if="latestRun.state === 'interrupted' && latestRun.resumable"
            class="text-(--ui-text-dimmed) text-xs"
          >
            Progress is saved. Resume picks up where it left off — courses
            already finished come straight from the cache.
          </p>
          <div
            v-else-if="latestRun.state === 'interrupted'"
            class="text-(--ui-text-muted) text-xs flex flex-col gap-0.5"
          >
            <span v-for="blocker in latestRun.resumeBlockers" :key="blocker">
              {{ resumeBlockerText(blocker) }}
            </span>
          </div>

          <p v-if="resumeRun.isError.value" class="text-(--ui-color-error-500) text-xs">
            Resume failed: {{ resumeRun.error.value?.message }}
          </p>

          <p v-if="latestRun.errorMessage" class="text-(--ui-color-error-500) text-xs">
            {{ latestRun.errorMessage }}
          </p>

          <div
            v-if="latestRun.state === 'completed' && latestRun.digitLevel && latestRun.digitLevel !== viewLevel"
          >
            <UButton
              variant="link"
              color="primary"
              size="xs"
              class="px-0"
              @click="viewLevel = latestRun.digitLevel as 2 | 4 | 6"
            >
              View {{ latestRun.digitLevel }}-digit results
            </UButton>
          </div>
        </div>
      </Transition>
    </section>

    <section v-if="!isImporting" class="flex flex-col gap-3 min-h-0">
      <div class="flex items-center justify-between gap-3 flex-wrap">
        <div class="flex items-center gap-3">
          <h3 class="text-sm font-medium text-(--ui-text)">Courses</h3>
          <!-- View switcher: which model's results the table shows. Safe to
               click — never starts work. The chip is that level's coverage. -->
          <div
            role="group"
            aria-label="Result digit level"
            class="inline-flex rounded-md border border-(--ui-border) bg-(--ui-bg-muted) p-0.5"
          >
            <button
              v-for="level in LEVELS"
              :key="level"
              type="button"
              class="px-2.5 py-1 rounded-[5px] text-xs flex items-baseline gap-1.5 transition-colors motion-reduce:transition-none"
              :class="
                viewLevel === level
                  ? 'bg-(--ui-bg) text-(--ui-text) font-medium shadow-sm'
                  : 'text-(--ui-text-muted) hover:text-(--ui-text)'
              "
              :aria-pressed="viewLevel === level"
              :aria-label="`${level}-digit results, ${coverageLabel(level) === '—' ? 'not classified' : `${coverageLabel(level)} classified`}`"
              @click="viewLevel = level"
            >
              {{ level }}-digit
              <span class="tabular-nums text-[10px] text-(--ui-text-dimmed)">
                {{ coverageLabel(level) }}
              </span>
            </button>
          </div>
        </div>
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
            @click="exportOpen = true"
          >
            Export CSV
          </UButton>
          <UModal v-model:open="exportOpen" :title="`Export ${viewLevel}-digit results`">
            <template #body>
              <div class="flex flex-col gap-3 text-sm">
                <p class="text-(--ui-text-muted)">
                  Exports every row of this dataset with the
                  {{ viewLevel }}-digit code, its probability, and the
                  standardized CCM title appended
                  (<code>ccm{{ viewLevel }}digit_code</code>,
                  <code>…_prob</code>, <code>…_title</code>).
                </p>
                <UCheckbox
                  v-model="includeTopCandidates"
                  label="Include top 5 candidate codes"
                  :description="`Adds ccm${viewLevel}digit_code1…5 with a probability and title per rank. Rank 1 repeats the main columns.`"
                />
              </div>
            </template>
            <template #footer>
              <div class="flex justify-end gap-2 w-full">
                <UButton variant="ghost" color="neutral" @click="exportOpen = false">
                  Cancel
                </UButton>
                <UButton color="primary" icon="i-lucide-download" @click="exportCsv">
                  Choose file…
                </UButton>
              </div>
            </template>
          </UModal>
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
                  {{ viewLevel }}-digit CCM
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
                            v-if="row.ccmTitleLevel === 2 && viewLevel !== 2"
                            class="text-xs text-(--ui-text-dimmed)"
                          >
                            2-digit parent category — no official
                            {{ viewLevel }}-digit title exists for this code.
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
