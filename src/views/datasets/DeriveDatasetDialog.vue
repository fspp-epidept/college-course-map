<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type {
  ColumnOrigin,
  DeriveRequest,
  FilterSpec,
  MappedNames,
  OutputColumn,
} from "../../bindings";
import FilterRows from "../../components/filter/FilterRows.vue";
import { useDatasets } from "../../composables/useDatasets";
import {
  useCreateDerivedDataset,
  useDerivePreview,
  useDerivedColumns,
} from "../../composables/useDerive";
import { useWorkspace } from "../../stores/workspace";

// Build a dataset from the rows of one or more datasets that match a filter
// (#254). Top to bottom: sources, filter, preview, columns, duplicates,
// title. Opened by Save as Dataset (seeded with a dataset and its filter)
// and New from Datasets (empty) through the workspace store. The backend
// re-checks everything; the checks here only keep Create honest.
const open = defineModel<boolean>("open", { required: true });

const workspace = useWorkspace();
const { data: datasets } = useDatasets();
const candidates = computed(() => (datasets.value ?? []).filter((d) => d.importState === "ready"));

// --- Sources, in the order they were checked ---
const sources = ref<string[]>([]);
const sourceInfo = computed(() =>
  sources.value.flatMap((id) => {
    const d = candidates.value.find((c) => c.id === id);
    return d ? [{ id: d.id, title: d.title }] : [];
  }),
);
const merge = computed(() => sources.value.length > 1);
function toggleSource(id: string, checked: boolean | "indeterminate"): void {
  const next = sources.value.filter((s) => s !== id);
  sources.value = checked === true ? [...next, id] : next;
}

// --- Filter ---
const filter = ref<FilterSpec>({ rows: [] });

// --- Columns ---
// Drafts mirror the default output columns and are re-seeded whenever the
// sources change. Giving two drafts the same name merges them into one
// output column; the backend's rules are checked here too so Create is
// disabled with the same message it would return.
interface ColumnDraft {
  key: string;
  keep: boolean;
  name: string;
  from: ColumnOrigin[];
}
const SOURCE_COLUMN = "source_dataset";
const mapped = ref<MappedNames>({
  subject: "subject_code",
  catalog: "catalog_number",
  title: "course_title",
});
const drafts = ref<ColumnDraft[]>([]);
const {
  data: defaults,
  error: defaultsError,
  isPending: defaultsPending,
} = useDerivedColumns(sources);
watch(defaults, (d) => {
  if (!d) return;
  mapped.value = { ...d.mapped };
  drafts.value = d.columns.map((c, i) => ({
    key: String(i),
    // Default: columns every source has are kept.
    keep: c.from.length === sources.value.length,
    name: c.name,
    from: c.from,
  }));
});
watch(sources, (s) => {
  if (s.length === 0) drafts.value = [];
});

function originFor(draft: ColumnDraft, sourceId: string): ColumnOrigin | undefined {
  return draft.from.find((o) => o.source === sourceId);
}

const outputColumns = computed<OutputColumn[]>(() => {
  const byName = new Map<string, OutputColumn>();
  for (const draft of drafts.value) {
    if (!draft.keep) continue;
    const name = draft.name.trim();
    const key = name.toLowerCase();
    const existing = byName.get(key);
    if (existing) existing.from.push(...draft.from);
    else byName.set(key, { name, from: [...draft.from] });
  }
  return [...byName.values()];
});

const outputNames = computed(() => [
  mapped.value.subject.trim(),
  mapped.value.catalog.trim(),
  mapped.value.title.trim(),
  ...outputColumns.value.map((c) => c.name),
  ...(merge.value ? [SOURCE_COLUMN] : []),
]);

const columnError = computed<string | null>(() => {
  const names = outputNames.value;
  if (names.some((n) => n === "")) return "Every column needs a name.";
  const userNamed = merge.value ? names.slice(0, -1) : names;
  if (userNamed.some((n) => n.toLowerCase() === SOURCE_COLUMN)) {
    return `“${SOURCE_COLUMN}” is reserved for the column that says which dataset a row came from.`;
  }
  const seen = new Set<string>();
  for (const name of names) {
    const key = name.toLowerCase();
    if (seen.has(key)) return `Two columns are both named “${name}”.`;
    seen.add(key);
  }
  for (const column of outputColumns.value) {
    const perSource = new Set<string>();
    for (const origin of column.from) {
      if (perSource.has(origin.source)) {
        const title = sourceInfo.value.find((s) => s.id === origin.source)?.title ?? origin.source;
        return `“${column.name}” takes two columns from “${title}”. A column can take one per dataset.`;
      }
      perSource.add(origin.source);
    }
  }
  return null;
});

// A filter row naming a column that was dropped or renamed can't be sent.
watch(outputColumns, (columns) => {
  const names = new Set(columns.map((c) => c.name));
  const rows = filter.value.rows.filter(
    (r) => r.field.kind !== "column" || names.has(r.field.name),
  );
  if (rows.length !== filter.value.rows.length) filter.value = { rows };
});

// --- Duplicates ---
const dedupeMode = ref<"all" | "columns">("all");
const dedupeColumns = ref<string[]>([]);
// Every kept column by default, including source_dataset: rows from
// different datasets never collapse unless the user unticks it.
watch(
  outputNames,
  (names) => {
    dedupeColumns.value = names.filter((n) => n !== "");
  },
  { immediate: true },
);
const dedupeItems = computed(() =>
  outputNames.value.filter((n) => n !== "").map((n) => ({ label: n, value: n })),
);

// --- Title ---
const title = ref("");
const suggestedTitle = computed(() => {
  const [first] = sourceInfo.value;
  if (!first) return "";
  return merge.value
    ? `${first.title} and ${sourceInfo.value.length - 1} more`
    : `${first.title} subset`;
});

// --- Request, preview, create ---
const request = computed<DeriveRequest | null>(() => {
  if (sources.value.length === 0 || columnError.value !== null) return null;
  return {
    title: title.value.trim() || suggestedTitle.value,
    sources: sources.value,
    filter: {
      rows: filter.value.rows.filter(
        (r) => r.values.length > 0 || r.op === "isEmpty" || r.op === "isNotEmpty",
      ),
    },
    mappedNames: {
      subject: mapped.value.subject.trim(),
      catalog: mapped.value.catalog.trim(),
      title: mapped.value.title.trim(),
    },
    columns: outputColumns.value,
    dedupeColumns:
      dedupeMode.value === "columns" && dedupeColumns.value.length > 0 ? dedupeColumns.value : null,
  };
});
// Debounced so typing in a name or value doesn't query per keystroke.
const debouncedRequest = ref<DeriveRequest | null>(null);
let debounce: ReturnType<typeof setTimeout> | undefined;
watch(
  request,
  (next) => {
    clearTimeout(debounce);
    debounce = setTimeout(() => {
      debouncedRequest.value = next;
    }, 300);
  },
  { immediate: true },
);
const {
  data: preview,
  isFetching: previewFetching,
  error: previewError,
} = useDerivePreview(debouncedRequest, open);

const create = useCreateDerivedDataset();
function onCreate(): void {
  const req = request.value;
  if (!req) return;
  create.mutate(req, {
    onSuccess: (datasetId) => {
      open.value = false;
      workspace.setActiveActivity("datasets");
      workspace.selectDataset(datasetId);
    },
  });
}
const createDisabled = computed(
  () => request.value === null || previewError.value !== null || create.isPending.value,
);

// Seed from the opener each time the dialog opens; everything else resets.
watch(open, (now) => {
  if (!now) return;
  sources.value = [...workspace.deriveSeed.sources];
  filter.value = {
    rows: workspace.deriveSeed.filter.rows.map((r) => ({ ...r, values: [...r.values] })),
  };
  title.value = "";
  dedupeMode.value = "all";
  create.reset();
});
</script>

<template>
  <UModal
    v-model:open="open"
    title="New dataset from datasets"
    description="Rows from one or more datasets that match a filter, with the columns you choose. The new dataset is a copy and does not change when its sources do."
    :ui="{ content: 'max-w-5xl' }"
  >
    <template #body>
      <div class="flex flex-col gap-6 text-sm">
        <!-- 1. Sources -->
        <section class="flex flex-col gap-2">
          <h3 class="font-medium text-(--ui-text)">Datasets</h3>
          <p v-if="candidates.length === 0" class="text-(--ui-text-dimmed)">
            No datasets are ready yet.
          </p>
          <div v-else class="flex flex-col gap-1">
            <UCheckbox
              v-for="d in candidates"
              :key="d.id"
              :model-value="sources.includes(d.id)"
              :label="d.title"
              :description="`${d.rowCount.toLocaleString()} courses`"
              @update:model-value="toggleSource(d.id, $event)"
            />
          </div>
          <p v-if="merge" class="text-xs text-(--ui-text-dimmed)">
            Rows come in this order: {{ sourceInfo.map((s) => s.title).join(", ") }}. A
            <code>source_dataset</code> column says which dataset each row came from.
          </p>
          <p v-if="defaultsError" class="text-(--ui-color-error-500)">
            {{ defaultsError.message }}
          </p>
        </section>

        <template v-if="sources.length > 0 && !defaultsError && !defaultsPending">
          <!-- 2. Filter -->
          <section class="flex flex-col gap-2">
            <h3 class="font-medium text-(--ui-text)">Filter</h3>
            <p class="text-xs text-(--ui-text-dimmed)">
              Every row matches when there are no filters. Values are matched ignoring case.
            </p>
            <FilterRows
              v-model="filter"
              :columns="outputColumns.map((c) => ({ name: c.name, header: c.name }))"
              :lookup-dataset-id="sources.length === 1 ? (sources[0] ?? null) : null"
              :sources="sourceInfo"
            />
          </section>

          <!-- 3. Preview -->
          <section class="flex flex-col gap-2">
            <div class="flex items-baseline gap-3 flex-wrap">
              <h3 class="font-medium text-(--ui-text)">Preview</h3>
              <span v-if="preview" class="text-(--ui-text-muted) tabular-nums">
                {{ preview.matched.toLocaleString() }}
                {{ preview.matched === 1 ? "row matches" : "rows match" }}
                <template v-if="merge">
                  ({{
                    preview.bySource
                      .map((s) => `${sourceInfo.find((i) => i.id === s.source)?.title ?? s.source}: ${s.matched.toLocaleString()}`)
                      .join(", ")
                  }})
                </template>
                <template v-if="dedupeMode === 'columns'"> before removing duplicates</template>
              </span>
              <span v-if="previewFetching" class="text-xs text-(--ui-text-dimmed)">Updating…</span>
            </div>
            <p v-if="previewError" class="text-(--ui-color-error-500)">
              {{ previewError.message }}
            </p>
            <div v-else-if="preview" class="rounded-lg border border-(--ui-border) overflow-x-auto max-h-64">
              <table class="min-w-full text-xs">
                <thead class="bg-(--ui-bg-muted)">
                  <tr>
                    <th
                      v-for="header in preview.headers"
                      :key="header"
                      class="px-3 py-2 text-left font-medium text-(--ui-text) whitespace-nowrap"
                    >
                      {{ header }}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-if="preview.rows.length === 0" class="text-(--ui-text-dimmed)">
                    <td :colspan="preview.headers.length" class="px-3 py-4 text-center">
                      No rows match.
                    </td>
                  </tr>
                  <tr
                    v-for="(row, i) in preview.rows"
                    :key="i"
                    class="border-t border-(--ui-border-muted)"
                  >
                    <td
                      v-for="(cell, j) in row"
                      :key="j"
                      class="px-3 py-1.5 text-(--ui-text) whitespace-nowrap"
                    >
                      {{ cell ?? "" }}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>

          <!-- 4. Columns -->
          <section class="flex flex-col gap-2">
            <h3 class="font-medium text-(--ui-text)">Columns</h3>
            <p class="text-xs text-(--ui-text-dimmed)">
              Subject, catalog number and title always carry over. Columns are matched across
              datasets by header; give two columns the same name to merge them.
            </p>
            <div class="rounded-lg border border-(--ui-border) overflow-x-auto">
              <table class="min-w-full text-xs">
                <thead class="bg-(--ui-bg-muted)">
                  <tr>
                    <th class="px-3 py-2 text-left font-medium text-(--ui-text) w-12">Keep</th>
                    <th class="px-3 py-2 text-left font-medium text-(--ui-text)">Name in new dataset</th>
                    <th
                      v-for="s in sourceInfo"
                      :key="s.id"
                      class="px-3 py-2 text-left font-medium text-(--ui-text) whitespace-nowrap"
                    >
                      From {{ s.title }}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="(field, i) in (['subject', 'catalog', 'title'] as const)"
                    :key="field"
                    class="border-t border-(--ui-border-muted)"
                  >
                    <td class="px-3 py-1.5 text-(--ui-text-dimmed)">always</td>
                    <td class="px-3 py-1.5">
                      <UInput v-model="mapped[field]" size="xs" :aria-label="`Name of the ${field} column`" />
                    </td>
                    <td
                      v-for="s in sourceInfo"
                      :key="s.id"
                      class="px-3 py-1.5 text-(--ui-text-muted)"
                    >
                      {{ ["subject code", "catalog number", "course title"][i] }}
                    </td>
                  </tr>
                  <tr
                    v-for="draft in drafts"
                    :key="draft.key"
                    class="border-t border-(--ui-border-muted)"
                  >
                    <td class="px-3 py-1.5">
                      <UCheckbox v-model="draft.keep" :aria-label="`Keep ${draft.name}`" />
                    </td>
                    <td class="px-3 py-1.5">
                      <UInput v-model="draft.name" size="xs" :disabled="!draft.keep" aria-label="Column name" />
                    </td>
                    <td
                      v-for="s in sourceInfo"
                      :key="s.id"
                      class="px-3 py-1.5 text-(--ui-text-muted)"
                    >
                      {{ originFor(draft, s.id)?.header ?? "—" }}
                    </td>
                  </tr>
                  <tr v-if="merge" class="border-t border-(--ui-border-muted)">
                    <td class="px-3 py-1.5 text-(--ui-text-dimmed)">always</td>
                    <td class="px-3 py-1.5 text-(--ui-text)"><code>source_dataset</code></td>
                    <td
                      v-for="s in sourceInfo"
                      :key="s.id"
                      class="px-3 py-1.5 text-(--ui-text-muted)"
                    >
                      “{{ s.title }}”
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p v-if="columnError" class="text-(--ui-color-error-500)">{{ columnError }}</p>
          </section>

          <!-- 5. Duplicates -->
          <section class="flex flex-col gap-2">
            <h3 class="font-medium text-(--ui-text)">Duplicates</h3>
            <URadioGroup
              v-model="dedupeMode"
              :items="[
                { value: 'all', label: 'Keep all rows' },
                {
                  value: 'columns',
                  label: 'Remove rows that repeat in the chosen columns',
                  description: 'The first row in dataset order is kept.',
                },
              ]"
            />
            <UCheckboxGroup
              v-if="dedupeMode === 'columns'"
              v-model="dedupeColumns"
              :items="dedupeItems"
              orientation="horizontal"
              class="ml-6"
            />
          </section>

          <!-- 6. Title -->
          <section class="flex flex-col gap-2">
            <h3 class="font-medium text-(--ui-text)">Title</h3>
            <UInput v-model="title" :placeholder="suggestedTitle" class="max-w-md" aria-label="Title" />
          </section>
        </template>

        <p v-if="create.error.value" class="text-(--ui-color-error-500)">
          Couldn't create the dataset: {{ create.error.value.message }}
        </p>
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="open = false">Cancel</UButton>
        <UButton color="primary" :disabled="createDisabled" :loading="create.isPending.value" @click="onCreate">
          Create Dataset
        </UButton>
      </div>
    </template>
  </UModal>
</template>
