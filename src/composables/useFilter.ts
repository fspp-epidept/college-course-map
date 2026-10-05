import { useQuery } from "@tanstack/vue-query";
import { type MaybeRefOrGetter, computed, toValue } from "vue";
import {
  type ColumnValue,
  type DatasetColumn,
  type FilterField,
  type FilterOp,
  type FilterSpec,
  commands,
} from "../bindings";

/** Operators that compare against values; the other two test for blank. */
export function opTakesValues(op: FilterOp): boolean {
  return op !== "isEmpty" && op !== "isNotEmpty";
}

/**
 * The rows the backend accepts: a value operator needs at least one non-blank
 * value. The editor keeps half-built rows so the user can finish them; only
 * complete rows are sent.
 */
export function completeRows(spec: FilterSpec): FilterSpec {
  return {
    rows: spec.rows.filter(
      (row) => !opTakesValues(row.op) || row.values.some((v) => v.trim() !== ""),
    ),
  };
}

/** A stable string for a field, for select models and query keys. */
export function fieldKey(field: FilterField): string {
  switch (field.kind) {
    case "column":
      return `column:${field.name}`;
    case "ccm":
      return `ccm:${field.digitLevel}`;
    default:
      return field.kind;
  }
}

export function parseFieldKey(key: string): FilterField {
  if (key.startsWith("column:")) return { kind: "column", name: key.slice("column:".length) };
  if (key.startsWith("ccm:")) return { kind: "ccm", digitLevel: Number(key.slice("ccm:".length)) };
  if (key === "subject" || key === "catalog" || key === "title" || key === "sourceDataset") {
    return { kind: key };
  }
  throw new Error(`unknown filter field ${key}`);
}

/**
 * The columns of a dataset a filter may name (`filter.rs::dataset_columns`):
 * its layout's non-mapped headers. Empty for a dataset with no stored layout.
 */
export function useDatasetColumns(datasetId: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ["datasetColumns", toValue(datasetId)] as const),
    queryFn: async (): Promise<DatasetColumn[]> => {
      const result = await commands.datasetColumns(toValue(datasetId));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

interface UseColumnValuesArgs {
  datasetId: MaybeRefOrGetter<string>;
  field: MaybeRefOrGetter<FilterField>;
  search: MaybeRefOrGetter<string>;
  enabled: MaybeRefOrGetter<boolean>;
}

/**
 * The value picker's list (`filter.rs::column_values`): a field's most
 * frequent distinct values in a dataset, narrowed by the search text. Keeps
 * the previous list while the next loads so the menu doesn't flash empty
 * per keystroke.
 */
export function useColumnValues(args: UseColumnValuesArgs) {
  return useQuery({
    queryKey: computed(
      () =>
        [
          "columnValues",
          toValue(args.datasetId),
          fieldKey(toValue(args.field)),
          toValue(args.search).trim().toLowerCase(),
        ] as const,
    ),
    queryFn: async (): Promise<ColumnValue[]> => {
      const result = await commands.columnValues({
        datasetId: toValue(args.datasetId),
        field: toValue(args.field),
        search: toValue(args.search),
      });
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    enabled: computed(() => toValue(args.enabled)),
    placeholderData: (prev) => prev,
    staleTime: 60_000,
  });
}
