<script setup lang="ts">
import { computed } from "vue";
import type { DatasetColumn, FilterField, FilterOp, FilterRow, FilterSpec } from "../../bindings";
import { fieldKey, opTakesValues, parseFieldKey } from "../../composables/useFilter";
import FilterValues from "./FilterValues.vue";

// The flat filter editor (#254): rows AND'd, values within a row OR'd. Pure
// UI over a `FilterSpec`; the backend compiles it. Half-built rows stay
// in the model, `completeRows` (useFilter) picks what is sent.
const props = defineProps<{
  /** Columns of the dataset(s) a filter may name, in layout order. */
  columns: DatasetColumn[];
  /** Dataset the value picker lists values from; null for typed values only. */
  lookupDatasetId: string | null;
  /** Source datasets, offered as a field when there are several. */
  sources?: { id: string; title: string }[];
}>();
const spec = defineModel<FilterSpec>({ required: true });

interface FieldItem {
  key: string;
  label: string;
}
const STANDARD_FIELDS: { field: FilterField; label: string }[] = [
  { field: { kind: "subject" }, label: "Subject code" },
  { field: { kind: "catalog" }, label: "Catalog number" },
  { field: { kind: "title" }, label: "Course title" },
  { field: { kind: "ccm", digitLevel: 2 }, label: "CCM 2-digit code" },
  { field: { kind: "ccm", digitLevel: 4 }, label: "CCM 4-digit code" },
  { field: { kind: "ccm", digitLevel: 6 }, label: "CCM 6-digit code" },
];
// Two groups: the standard fields, then the columns.
const fieldItems = computed<FieldItem[][]>(() => {
  const standard: FieldItem[] = STANDARD_FIELDS.map((f) => ({
    key: fieldKey(f.field),
    label: f.label,
  }));
  if ((props.sources?.length ?? 0) > 1) {
    standard.push({ key: fieldKey({ kind: "sourceDataset" }), label: "Source dataset" });
  }
  const columns: FieldItem[] = props.columns.map((c) => ({
    key: fieldKey({ kind: "column", name: c.name }),
    label: c.header,
  }));
  return columns.length > 0 ? [standard, columns] : [standard];
});

const OP_LABELS: Record<FilterOp, string> = {
  is: "is any of",
  isNot: "is none of",
  contains: "contains",
  notContains: "doesn't contain",
  startsWith: "starts with",
  isEmpty: "is empty",
  isNotEmpty: "is not empty",
};
function opsFor(field: FilterField): { value: FilterOp; label: string }[] {
  const ops: FilterOp[] =
    field.kind === "sourceDataset"
      ? ["is", "isNot"]
      : field.kind === "ccm"
        ? ["is", "isNot", "contains", "notContains", "startsWith"]
        : ["is", "isNot", "contains", "notContains", "startsWith", "isEmpty", "isNotEmpty"];
  return ops.map((value) => ({ value, label: OP_LABELS[value] }));
}

function replace(index: number, row: FilterRow): void {
  spec.value = { rows: spec.value.rows.map((r, i) => (i === index ? row : r)) };
}
function setField(index: number, key: unknown): void {
  if (typeof key !== "string") return;
  replace(index, { field: parseFieldKey(key), op: "is", values: [] });
}
function setOp(index: number, op: unknown): void {
  const row = spec.value.rows[index];
  if (!row || typeof op !== "string") return;
  const next = op as FilterOp;
  replace(index, { ...row, op: next, values: opTakesValues(next) ? row.values : [] });
}
function setValues(index: number, values: string[]): void {
  const row = spec.value.rows[index];
  if (!row) return;
  replace(index, { ...row, values });
}
function add(): void {
  spec.value = { rows: [...spec.value.rows, { field: { kind: "subject" }, op: "is", values: [] }] };
}
function remove(index: number): void {
  spec.value = { rows: spec.value.rows.filter((_, i) => i !== index) };
}
function clear(): void {
  spec.value = { rows: [] };
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div
      v-for="(row, i) in spec.rows"
      :key="i"
      class="flex items-start gap-2 flex-wrap"
    >
      <USelectMenu
        :model-value="fieldKey(row.field)"
        :items="fieldItems"
        value-key="key"
        label-key="label"
        size="sm"
        class="w-56"
        aria-label="Field"
        @update:model-value="setField(i, $event)"
      />
      <USelect
        :model-value="row.op"
        :items="opsFor(row.field)"
        value-key="value"
        label-key="label"
        size="sm"
        class="w-40"
        aria-label="Operator"
        @update:model-value="setOp(i, $event)"
      />
      <FilterValues
        v-if="opTakesValues(row.op)"
        :model-value="row.values"
        :field="row.field"
        :lookup-dataset-id="lookupDatasetId"
        :sources="sources"
        class="flex-1 min-w-56"
        @update:model-value="setValues(i, $event)"
      />
      <UButton
        icon="i-lucide-x"
        variant="ghost"
        color="neutral"
        size="sm"
        aria-label="Remove filter"
        @click="remove(i)"
      />
      <p v-if="row.field.kind === 'ccm'" class="basis-full text-xs text-(--ui-text-dimmed)">
        Matches only courses already classified at this level.
      </p>
    </div>
    <div class="flex items-center gap-2">
      <UButton icon="i-lucide-plus" variant="ghost" color="neutral" size="sm" @click="add">
        Add Filter
      </UButton>
      <UButton v-if="spec.rows.length > 0" variant="ghost" color="neutral" size="sm" @click="clear">
        Clear Filters
      </UButton>
    </div>
  </div>
</template>
