<script setup lang="ts">
import { computed, ref } from "vue";
import type { FilterField } from "../../bindings";
import { useColumnValues } from "../../composables/useFilter";

// The values control of one filter row. With a dataset to look values up
// in, it is a searchable multi-select of the field's distinct values with
// counts (typed values can still be added); the source-dataset field lists
// the sources; otherwise it is plain tags.
const props = defineProps<{
  field: FilterField;
  /** Dataset whose distinct values are offered; null for typed values only. */
  lookupDatasetId: string | null;
  sources?: { id: string; title: string }[];
}>();
const values = defineModel<string[]>({ required: true });

const search = ref("");
const pickable = computed(
  () => props.lookupDatasetId !== null && props.field.kind !== "sourceDataset",
);
const { data: found, isFetching } = useColumnValues({
  datasetId: () => props.lookupDatasetId ?? "",
  field: () => props.field,
  search,
  enabled: pickable,
});

interface ValueItem {
  value: string;
  label: string;
  count: number | null;
}

// Selected values that the current search doesn't list are kept as items so
// their chips still render.
const items = computed<ValueItem[]>(() => {
  if (props.field.kind === "sourceDataset") {
    return (props.sources ?? []).map((s) => ({ value: s.id, label: s.title, count: null }));
  }
  const listed: ValueItem[] = (found.value ?? []).map((v) => ({
    value: v.value,
    label: v.label ? `${v.value}  ${v.label}` : v.value,
    count: v.count,
  }));
  const known = new Set(listed.map((i) => i.value));
  const extra = values.value
    .filter((v) => !known.has(v))
    .map((v) => ({ value: v, label: v, count: null }));
  return [...listed, ...extra];
});

function onCreate(item: string): void {
  const value = item.trim();
  if (value === "" || values.value.includes(value)) return;
  values.value = [...values.value, value];
}
</script>

<template>
  <USelectMenu
    v-if="pickable || field.kind === 'sourceDataset'"
    v-model="values"
    v-model:search-term="search"
    :items="items"
    value-key="value"
    label-key="label"
    multiple
    size="sm"
    :loading="isFetching"
    :create-item="pickable ? 'always' : false"
    :placeholder="field.kind === 'sourceDataset' ? 'Choose datasets' : 'Choose or type values'"
    @create="onCreate"
  >
    <template #item-trailing="{ item }">
      <span v-if="item.count !== null" class="text-xs text-(--ui-text-dimmed) tabular-nums">
        {{ item.count.toLocaleString() }}
      </span>
    </template>
  </USelectMenu>
  <UInputTags
    v-else
    v-model="values"
    size="sm"
    add-on-blur
    placeholder="Type a value and press Enter"
  />
</template>
