<script setup lang="ts">
import { useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { type ClassifyState, type FilterSpec, commands } from "../../bindings";
import FilterRows from "../../components/filter/FilterRows.vue";
import InputProfilePanel from "../../components/InputProfilePanel.vue";
import InputSamples from "../../components/InputSamples.vue";
import { INPUT_FINDINGS } from "../../config/inputFindings";
import { useCourses, useCoverage, useModelIdForDigitLevel } from "../../composables/useCourses";
import { useDatasets, useInputProfile } from "../../composables/useDatasets";
import { useDerivation } from "../../composables/useDerive";
import { completeRows, describeFilter, useDatasetColumns } from "../../composables/useFilter";
import {
  progressDone,
  progressTotal,
  useClassifyDataset,
  useClassifyRate,
  useStopClassification,
} from "../../composables/useClassify";
import { useWorkspace } from "../../stores/workspace";
import DeleteDatasetDialog from "./DeleteDatasetDialog.vue";
// Master/detail (EPI-58): DatasetsPanel keys this component by dataset id, so
// all local state (view level, cursors, dialogs) is per-dataset by
// construction.
const props = defineProps<{ datasetId: string }>();

const currentDatasetId = computed(() => props.datasetId);

type DigitLevel = 2 | 4 | 6;
const LEVELS: readonly DigitLevel[] = [2, 4, 6] as const;

// Which model's results the courses table shows. Pure view state — switching
// never starts work (EPI-68 view/action split). Fresh per dataset:
// DatasetsPanel keys this component by dataset id.
const viewLevel = ref<DigitLevel>(6);

const queryClient = useQueryClient();

// --- Dataset import state ---
// Surface this dataset's import state by reusing the cached datasets query
// (it's already polled while any import is active). No extra IPC traffic.
const { data: datasets } = useDatasets();
const dataset = computed(() => datasets.value?.find((d) => d.id === currentDatasetId.value));
const isImporting = computed(() => dataset.value?.importState === "importing");
const importFailed = computed(() => dataset.value?.importState === "failed");
// A delete in flight, or one that was cut off and waits to be finished
// (#199). Either way the dataset is closed to new work.
const isDeleting = computed(() => dataset.value?.importState === "deleting");
const deleteIncomplete = computed(() => dataset.value?.importState === "delete_incomplete");
const beingDeleted = computed(() => isDeleting.value || deleteIncomplete.value);

// When import finishes (or fails), refresh the courses + coverage queries
// exactly once so the table fills in, and the input profile the worker
// stored on completion.
watch(isImporting, (now, before) => {
  if (before && !now) {
    queryClient.invalidateQueries({ queryKey: ["courses", currentDatasetId.value] });
    queryClient.invalidateQueries({ queryKey: ["coverage", currentDatasetId.value] });
    queryClient.invalidateQueries({ queryKey: ["inputProfile", currentDatasetId.value] });
  }
});

// --- Classification (#249) ---
// The dataset's classification state and, while a job runs, its live
// progress ride on the datasets list, which polls every 500 ms meanwhile.
const classification = computed(() => dataset.value?.classification);
const isRunning = computed(() => classification.value?.state === "running");
const progress = computed(() => classification.value?.progress ?? null);
const stopping = computed(() => progress.value?.stopping ?? false);
// Only one dataset classifies at a time, app-wide.
const activeElsewhere = computed(
  () =>
    datasets.value?.find(
      (d) => d.classification.state === "running" && d.id !== currentDatasetId.value,
    ) ?? null,
);

// --- Coverage (EPI-68) ---
// Per-model classified/total counts from the cache, at rest. While a job
// runs, its in-memory progress stands in (same units), so the 500 ms poll
// never turns into coverage scans.
const { data: coverage, refetch: refetchCoverage } = useCoverage(currentDatasetId);
function levelCounts(level: DigitLevel): { done: number; total: number } | null {
  const live = progress.value?.levels.find((l) => l.digitLevel === level);
  if (live) return { done: live.done, total: live.total };
  const c = coverage.value?.find((row) => row.digitLevel === level);
  return c ? { done: c.classified, total: c.total } : null;
}
function coverageLabel(level: DigitLevel): string {
  const c = levelCounts(level);
  if (!c || c.total === 0 || c.done === 0) return "—";
  const pct = Math.floor((c.done / c.total) * 100);
  // A dataset that's classified-but-not-quite-100% floors to 99, never
  // rounds up to a dishonest 100.
  return `${c.done >= c.total ? 100 : Math.min(pct, 99)}%`;
}
const fullyClassified = computed(() =>
  LEVELS.every((level) => {
    const c = levelCounts(level);
    return c !== null && c.total > 0 && c.done >= c.total;
  }),
);
const anyClassified = computed(() => LEVELS.some((level) => (levelCounts(level)?.done ?? 0) > 0));

// --- Classify action ---
// One button, one confirm: a job always covers every model, and classifying
// a stopped dataset again is the resume (the cache skips finished courses).

const confirmOpen = ref(false);
const classify = useClassifyDataset();

function requestClassify(): void {
  classify.reset();
  confirmOpen.value = true;
  // The confirm panel quotes cache numbers; make sure they're current at the
  // moment of decision, not from page-mount time.
  refetchCoverage();
}

function startClassify(): void {
  classify.mutate(currentDatasetId.value, {
    onSuccess: () => {
      confirmOpen.value = false;
    },
  });
}

// Confirm-panel numbers: what each level still needs to compute.
const confirmLevels = computed(() =>
  LEVELS.map((level) => {
    const c = coverage.value?.find((row) => row.digitLevel === level);
    return {
      level,
      remaining: c ? c.total - c.classified : null,
      classified: c?.classified ?? 0,
    };
  }),
);

// --- Stop ---
const stopClassification = useStopClassification();
function onStop(): void {
  stopClassification.mutate(currentDatasetId.value);
}

// --- Input check ---
// The profile the import worker persisted (profile.rs). Null for datasets
// imported before input checks existed; those say so rather than recompute.
const { data: inputProfile, isPending: inputProfilePending } = useInputProfile(currentDatasetId);
const profileOpen = ref(false);
const profileWarnings = computed(
  () => inputProfile.value?.findings.filter((f) => f.severity === "warning") ?? [],
);
const profileSummary = computed(() => {
  const p = inputProfile.value;
  if (!p) return null;
  if (p.findings.length === 0) return "Input check: no issues";
  const warnings = profileWarnings.value.length;
  const notes = p.findings.length - warnings;
  const parts: string[] = [];
  if (warnings > 0) parts.push(`${warnings} ${warnings === 1 ? "warning" : "warnings"}`);
  if (notes > 0) parts.push(`${notes} ${notes === 1 ? "note" : "notes"}`);
  return `Input check: ${parts.join(", ")}`;
});

// Why Classify can't start right now (null = it can). The button's disabled
// state and the Classify menu's toast both read this.
const classifyBlocker = computed<string | null>(() => {
  if (beingDeleted.value) return "This dataset is being deleted.";
  if (isImporting.value) return "The import is still running.";
  if (importFailed.value) return "The import failed.";
  if (isRunning.value) return "This dataset is already classifying.";
  if (activeElsewhere.value) return `${activeElsewhere.value.title} is classifying.`;
  if (classify.isPending.value) return "Classification is already starting.";
  return null;
});
const classifyDisabled = computed(() => classifyBlocker.value !== null);

// --- Delete (#199) ---
// Why Delete can't start right now (null = it can); see classifyBlocker.
// The backend enforces the same rules.
const deleteOpen = ref(false);
const deleteBlocker = computed<string | null>(() => {
  if (isDeleting.value) return "This dataset is already being deleted.";
  if (isImporting.value) return "The import is still running.";
  if (isRunning.value) return "This dataset is classifying. Stop it first.";
  return null;
});

// --- Classification card presentation ---

const progressPct = computed(() => {
  const p = progress.value;
  const total = p ? progressTotal(p) : 0;
  if (!p || total === 0) return null;
  return Math.floor((progressDone(p) / total) * 100);
});

const rate = useClassifyRate(progress);
const rateLabel = computed(() =>
  rate.value === null ? null : `≈ ${Math.round(rate.value).toLocaleString()} classifications/s`,
);

// The model the job is on: the first one not yet complete.
const runningLevel = computed(
  () => progress.value?.levels.find((l) => l.done < l.total)?.digitLevel ?? null,
);

function fmtTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

const STATE_HEADINGS: Record<ClassifyState, () => string> = {
  running: () =>
    stopping.value
      ? "Stopping — finishing the current batch…"
      : runningLevel.value
        ? `Classifying with the ${runningLevel.value}-digit model…`
        : "Classifying…",
  stopped: () => "Classification stopped",
  failed: () => "Classification failed",
  idle: () =>
    fullyClassified.value
      ? "Classified with all models"
      : anyClassified.value
        ? "Partly classified"
        : "Not classified",
};
const heading = computed(() => {
  const state = classification.value?.state;
  return state ? STATE_HEADINGS[state]() : "";
});

// While a job is writing results for the level being viewed, keep the
// visible table page fresh. Watching that level's own counter (not the
// running level) catches its last window too. Other levels' columns can't
// change, so don't refetch them.
watch(
  () => progress.value?.levels.find((l) => l.digitLevel === viewLevel.value)?.done ?? null,
  (next, prev) => {
    if (next !== null && prev !== null && next !== prev) {
      queryClient.invalidateQueries({ queryKey: ["courses", currentDatasetId.value] });
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

// --- Filter (#254) ---
// The editor's rows, including half-built ones; only complete rows reach
// the query. The same spec will back Save as Dataset.
const filter = ref<FilterSpec>({ rows: [] });
const activeFilter = computed(() => completeRows(filter.value));
const isFiltered = computed(() => activeFilter.value.rows.length > 0);
const { data: datasetColumns } = useDatasetColumns(currentDatasetId);

// --- Derived datasets (#254) ---
const isDerived = computed(() => dataset.value?.sourceKind === "derived");
const { data: derivation } = useDerivation(currentDatasetId, isDerived);
const derivationFilter = computed(() =>
  derivation.value ? describeFilter(derivation.value.filter) : [],
);
// Why Save as Dataset can't open right now (null = it can); see classifyBlocker.
// A dataset with no stored layout is refused by the dialog, which says so.
const saveBlocker = computed<string | null>(() => {
  if (beingDeleted.value) return "This dataset is being deleted.";
  if (isImporting.value) return "The import is still running.";
  if (importFailed.value) return "The import failed.";
  return null;
});

// Reset pagination whenever the user switches digit level (the joined column
// changes underneath them) or the filter changes (the row set does).
watch([viewLevel, () => JSON.stringify(activeFilter.value)], () => {
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
  datasetId: currentDatasetId,
  modelId: computed(() => modelId.value ?? null),
  cursor,
  pageSize: PAGE_SIZE,
  filter: activeFilter,
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

// --- CSV export (EPI-15, EPI-77/78/79/80/81/98) ---
// The Export button opens a small options dialog; confirming closes it and
// hands off to Rust, which owns the save dialog and streams straight from
// DuckDB to disk. The resolved path comes back for display. `null` data
// means the user cancelled the save dialog.

const exporting = ref(false);
const exportOutcome = ref<{ path: string; rows: number } | null>(null);
const exportError = ref<string | null>(null);
const exportOpen = ref(false);
// Column/row options are explicit opt-ins: top-5 adds 15 columns per level
// (EPI-98), all-levels adds a ccm column set per digit level (EPI-80), and
// unique-rows collapses duplicates and drops the non-input columns (EPI-78).
const includeTopCandidates = ref(false);
const includeAllLevels = ref(false);
const uniqueRows = ref(false);

// Why Export can't open right now (null = it can); see classifyBlocker.
const exportBlocker = computed<string | null>(() => {
  if (beingDeleted.value) return "This dataset is being deleted.";
  if (modelId.value == null) return "The models aren't loaded yet.";
  if (isRunning.value) return "Wait for classification to finish.";
  if (totalRows.value === 0) return "There are no courses to export.";
  return null;
});

async function exportCsv(): Promise<void> {
  if (modelId.value == null) return;
  exportOpen.value = false;
  exporting.value = true;
  exportError.value = null;
  try {
    // Resolve the model id for each exported level through the catalog —
    // the viewed level's id is already cached; the all-levels case asks for
    // the other two at click time.
    const levels = includeAllLevels.value ? LEVELS : [viewLevel.value];
    const modelIds: number[] = [];
    for (const level of levels) {
      const resolved = await commands.modelIdForDigitLevel(level);
      if (resolved.status === "error") throw new Error(resolved.error);
      if (resolved.data == null) {
        throw new Error(`no active model for the ${level}-digit level`);
      }
      modelIds.push(resolved.data);
    }
    const result = await commands.exportResults({
      datasetId: currentDatasetId.value,
      modelIds,
      includeTopCandidates: includeTopCandidates.value,
      rowMode: uniqueRows.value ? "unique" : "all",
    });
    if (result.status === "error") throw new Error(result.error);
    if (result.data) exportOutcome.value = result.data;
  } catch (e) {
    exportError.value = (e as Error).message;
  } finally {
    exporting.value = false;
  }
}

// --- Menu requests (useNativeMenu) ---
// File → Export Results and Classify → Start Classification ask the selected
// dataset for its Classify / Export button action. Wait until the queries
// behind the blockers have settled, then do exactly what the button would.
const workspace = useWorkspace();
const toast = useToast();
const blockersSettled = computed(
  () =>
    dataset.value !== undefined &&
    (isImporting.value ||
      coursesError.value ||
      (modelId.value !== undefined && coursePage.value !== undefined)),
);
watch(
  [() => workspace.pendingDatasetAction, blockersSettled],
  ([action, settled]) => {
    if (action === null || !settled) return;
    workspace.pendingDatasetAction = null;
    const blocker = action === "classify" ? classifyBlocker.value : exportBlocker.value;
    if (blocker !== null) {
      const title = action === "classify" ? "Can't start classification" : "Can't export yet";
      toast.add({ title, description: blocker, color: "neutral" });
    } else if (action === "classify") {
      requestClassify();
    } else {
      exportOpen.value = true;
    }
  },
  { immediate: true },
);
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-6 overflow-auto">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-database" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ dataset?.title ?? "Dataset" }}</h2>
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
      <span
        v-else-if="isDeleting"
        class="text-(--ui-color-info-500) animate-pulse text-xs uppercase tracking-wide"
      >
        deleting
      </span>
      <span
        v-else-if="deleteIncomplete"
        class="text-(--ui-color-warning-500) text-xs uppercase tracking-wide"
      >
        delete incomplete
      </span>
      <UButton
        v-if="dataset && !isDeleting"
        class="ml-auto"
        color="error"
        variant="outline"
        size="xs"
        icon="i-lucide-trash-2"
        :disabled="deleteBlocker !== null"
        :title="deleteBlocker ?? undefined"
        @click="deleteOpen = true"
      >
        {{ deleteIncomplete ? "Finish Deleting…" : "Delete Dataset…" }}
      </UButton>
    </header>

    <DeleteDatasetDialog
      v-if="dataset"
      v-model:open="deleteOpen"
      :dataset-id="dataset.id"
      :title="dataset.title"
      :course-count="dataset.rowCount"
      :incomplete="deleteIncomplete"
    />

    <div
      v-if="isDeleting"
      class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-1"
    >
      <span class="text-(--ui-text) font-medium">Deleting this dataset…</span>
      <span class="text-(--ui-text-dimmed) text-xs">
        A large dataset can take a minute. You can keep working elsewhere in the app.
      </span>
    </div>
    <div
      v-else-if="deleteIncomplete"
      class="rounded-lg border border-(--ui-color-warning-500)/40 bg-(--ui-color-warning-500)/10 px-4 py-3 text-sm flex flex-col gap-1"
    >
      <span class="text-(--ui-text) font-medium">Deleting this dataset didn't finish</span>
      <span class="text-(--ui-text-muted)">
        The app closed before the delete was done. Its courses are already
        gone. Use Finish Deleting to remove what is left.
      </span>
    </div>

    <div
      v-if="isImporting && isDerived"
      class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-1"
    >
      <span class="text-(--ui-text) font-medium">Building this dataset…</span>
      <span class="text-(--ui-text-dimmed) text-xs">
        Copying the matching rows from the source datasets. A few seconds per
        million rows; the page fills in when it is done.
      </span>
    </div>
    <div
      v-else-if="isImporting"
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

    <!-- Created from: how a derived dataset was built. Read-only; the
         dataset is a copy and nothing re-runs when its sources change. -->
    <div
      v-if="isDerived && derivation && !isImporting"
      class="rounded-lg border border-(--ui-border) px-4 py-3 text-sm flex flex-col gap-1"
    >
      <span class="text-(--ui-text) font-medium">Created from</span>
      <ul class="text-(--ui-text-muted)">
        <li v-for="source in derivation.sources" :key="source.id">
          {{ source.title }}
          <span v-if="!source.exists" class="text-(--ui-text-dimmed)">(deleted)</span>
        </li>
      </ul>
      <template v-if="derivationFilter.length > 0">
        <span class="text-(--ui-text) mt-1">Filter</span>
        <ul class="text-(--ui-text-muted)">
          <li v-for="(line, i) in derivationFilter" :key="i">{{ line }}</li>
        </ul>
      </template>
      <span v-else class="text-(--ui-text-dimmed)">No filter: every row of the sources.</span>
      <span class="text-(--ui-text-muted)">
        <template v-if="derivation.dedupeColumns">
          Duplicates removed by: {{ derivation.dedupeColumns.join(", ") }}
        </template>
        <template v-else>All rows kept.</template>
      </span>
    </div>

    <div
      v-else-if="importFailed && dataset?.importError"
      class="rounded-lg border border-(--ui-color-error-500)/40 bg-(--ui-color-error-500)/10 px-4 py-3 text-sm text-(--ui-color-error-500)"
    >
      Import failed: {{ dataset.importError }}
    </div>

    <!-- Input check: the profile the import worker stored. The banner above
         covers the importing state; nothing renders until the query settles. -->
    <div
      v-if="!isImporting && !inputProfilePending && dataset?.importState === 'ready'"
      class="rounded-lg border border-(--ui-border) px-4 py-3 text-sm flex flex-col gap-2"
    >
      <template v-if="inputProfile">
        <button
          type="button"
          class="flex items-center gap-2 text-left text-(--ui-text) cursor-pointer"
          :aria-expanded="profileOpen"
          @click="profileOpen = !profileOpen"
        >
          <UIcon
            :name="profileOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
            class="size-4 text-(--ui-text-muted)"
          />
          <span class="font-medium">{{ profileSummary }}</span>
          <span
            v-if="profileWarnings.length > 0"
            class="text-xs text-(--ui-color-warning-500)"
          >
            {{ profileWarnings.map((f) => INPUT_FINDINGS[f.code].title).join(" · ") }}
          </span>
        </button>
        <InputProfilePanel v-if="profileOpen" :profile="inputProfile" />
      </template>
      <p v-else class="text-(--ui-text-muted)">
        Input check: not available. This dataset was imported before input checks existed;
        re-import the file to check it.
      </p>
    </div>

    <section v-if="!beingDeleted" class="flex flex-col gap-3">
      <!-- Classify action: one button, one confirm (a job always covers
           every model). Selection of what to LOOK at lives in the table
           header below; this control only starts work. -->
      <div class="flex items-center gap-3">
        <UButton
          color="primary"
          icon="i-lucide-play"
          :disabled="classifyDisabled"
          @click="requestClassify"
        >
          Classify
        </UButton>
        <span v-if="activeElsewhere" class="text-xs text-(--ui-text-muted)">
          <span class="text-(--ui-text)">{{ activeElsewhere.title }}</span>
          is classifying — stop it or wait for it to finish.
        </span>
      </div>

      <!-- Inline confirm panel (EPI-66's confirm requirement, EPI-68's form).
           Expands in place of the classification card; the numbers sit next to the table
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
            Classify {{ dataset?.title ?? "this dataset" }} with all models
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
          <!-- What the model will read (decision 2026-09-30): the stored
               profile's samples and its warnings, so a bad mapping is visible
               at the moment of decision. Informational; Classify stays on. -->
          <div class="flex flex-col gap-1 text-xs">
            <template v-if="inputProfile">
              <p class="text-(--ui-text-muted)">
                Classification uses the subject code, catalog number, and course title of each
                row. Examples from this dataset:
              </p>
              <InputSamples :samples="inputProfile.samples" />
              <template v-if="profileWarnings.length > 0">
                <ul class="text-(--ui-color-warning-500)">
                  <li v-for="f in profileWarnings" :key="f.code">
                    {{ INPUT_FINDINGS[f.code].title }} ({{ f.count.toLocaleString() }}
                    {{ f.rate === 0 ? "distinct values" : "rows" }})
                  </li>
                </ul>
                <p class="text-(--ui-text-dimmed)">Details are in the input check above.</p>
              </template>
            </template>
            <p v-else-if="!inputProfilePending" class="text-(--ui-text-dimmed)">
              This dataset has no input check; re-import the file to see one.
            </p>
          </div>
          <p class="text-(--ui-text-dimmed) text-xs">
            Runs locally on this machine, one model at a time. You can keep
            working while it runs.
          </p>
          <p v-if="classify.error.value" class="text-(--ui-color-error-500) text-xs">
            {{ classify.error.value.message }}
          </p>
          <div class="flex justify-end gap-2 pt-1">
            <UButton variant="ghost" color="neutral" @click="confirmOpen = false">
              Cancel
            </UButton>
            <UButton
              color="primary"
              :loading="classify.isPending.value"
              :disabled="classifyDisabled"
              @click="startClassify"
            >
              Classify
            </UButton>
          </div>
        </div>

        <!-- Classification card: the dataset's state as the backend reports
             it, with per-model coverage as the status. Confirm panel takes
             its place while a decision is pending. -->
        <div
          v-else-if="classification && dataset?.importState === 'ready'"
          key="classification-card"
          class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-2"
        >
          <div class="flex items-center justify-between gap-3">
            <span class="text-(--ui-text) font-medium">{{ heading }}</span>
            <div v-if="isRunning" class="flex items-center gap-3">
              <span v-if="progressPct !== null" class="tabular-nums text-(--ui-text-muted)">
                {{ progressPct }}%
              </span>
              <UButton
                color="neutral"
                variant="subtle"
                size="xs"
                icon="i-lucide-square"
                :disabled="stopping"
                @click="onStop"
              >
                {{ stopping ? "Stopping…" : "Stop" }}
              </UButton>
            </div>
          </div>

          <UProgress
            v-if="isRunning && progressPct !== null"
            :model-value="progressPct"
            :max="100"
            color="primary"
            size="sm"
          />

          <div class="grid grid-cols-2 gap-x-6 gap-y-1 text-(--ui-text-muted)">
            <template v-for="level in LEVELS" :key="level">
              <span>{{ level }}-digit model</span>
              <span class="text-(--ui-text) tabular-nums">
                <template v-if="levelCounts(level)">
                  {{ levelCounts(level)!.done.toLocaleString() }} /
                  {{ levelCounts(level)!.total.toLocaleString() }} courses
                </template>
                <template v-else>—</template>
              </span>
            </template>
            <template v-if="isRunning && rateLabel">
              <span>Throughput</span>
              <span class="text-(--ui-text) tabular-nums">{{ rateLabel }}</span>
            </template>
            <template v-if="classification.executionProvider">
              <span>Ran on</span>
              <span class="text-(--ui-text)">{{ classification.executionProvider }}</span>
            </template>
            <template v-if="classification.updatedAt && !isRunning">
              <span>Updated</span>
              <span class="text-(--ui-text)">{{ fmtTime(classification.updatedAt) }}</span>
            </template>
          </div>

          <p v-if="classification.state === 'stopped'" class="text-(--ui-text-dimmed) text-xs">
            Classify picks up where it left off; courses already finished come
            straight from the cache.
          </p>
          <p v-if="classification.state === 'failed' && classification.error" class="text-(--ui-color-error-500) text-xs">
            {{ classification.error }}
          </p>
        </div>
      </Transition>
    </section>

    <section v-if="!isImporting && !beingDeleted" class="flex flex-col gap-3 min-h-0">
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
              {{ pageRows.length.toLocaleString() }} of {{ totalRows.toLocaleString() }}{{ isFiltered ? " matching" : "" }}
            </template>
            <template v-else-if="isFiltered">No courses match</template>
          </span>
          <UButton
            variant="outline"
            color="neutral"
            icon="i-lucide-copy-plus"
            size="xs"
            :disabled="saveBlocker !== null"
            @click="workspace.openDeriveDialog([currentDatasetId], activeFilter)"
          >
            Save as Dataset
          </UButton>
          <UButton
            variant="outline"
            color="neutral"
            icon="i-lucide-download"
            size="xs"
            :loading="exporting"
            :disabled="exportBlocker !== null"
            @click="exportOpen = true"
          >
            Export CSV
          </UButton>
          <UModal
            v-model:open="exportOpen"
            :title="includeAllLevels ? 'Export all-level results' : `Export ${viewLevel}-digit results`"
          >
            <template #body>
              <div class="flex flex-col gap-3 text-sm">
                <p class="text-(--ui-text-muted)">
                  Exports this dataset with each selected level's code,
                  probability, and standardized CCM title appended
                  (<code>ccm{{ includeAllLevels ? "…" : viewLevel }}digit_code</code>,
                  <code>…_prob</code>, <code>…_title</code>). Levels without
                  classifications export as empty cells.
                </p>
                <UCheckbox
                  v-model="includeAllLevels"
                  label="Include all digit levels"
                  description="One column set per level (2, 4, and 6-digit) in a single file, instead of only the level in view."
                />
                <UCheckbox
                  v-model="uniqueRows"
                  label="One row per unique course"
                  description="Collapses duplicate courses to their first occurrence. Only the subject, catalog number, and title columns are kept — other columns differ between duplicates."
                />
                <UCheckbox
                  v-model="includeTopCandidates"
                  label="Include top 5 candidate codes"
                  description="Adds numbered code/probability/title columns per rank for each level. Rank 1 repeats the main columns."
                />
              </div>
            </template>
            <template #footer>
              <div class="flex justify-end gap-2 w-full">
                <UButton variant="ghost" color="neutral" @click="exportOpen = false">
                  Cancel
                </UButton>
                <UButton color="primary" icon="i-lucide-download" @click="exportCsv">
                  Choose File…
                </UButton>
              </div>
            </template>
          </UModal>
        </div>
      </div>

      <FilterRows
        v-model="filter"
        :columns="datasetColumns ?? []"
        :lookup-dataset-id="currentDatasetId"
      />

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
                  {{ isFiltered ? "No courses match the filter." : "No courses in this dataset." }}
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
                        <p v-else-if="viewLevel === 4" class="text-(--ui-text-dimmed) text-xs">
                          The CCM publishes no 4-digit titles.
                        </p>
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
      Dataset id: <code>{{ currentDatasetId }}</code>
    </p>
  </div>
</template>
