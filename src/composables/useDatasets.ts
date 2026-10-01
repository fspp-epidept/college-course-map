import { useQuery } from "@tanstack/vue-query";
import { type MaybeRefOrGetter, computed, toValue } from "vue";
import { type DatasetSummary, type InputProfile, commands } from "../bindings";

/**
 * Read the dataset list via the Rust `list_datasets` IPC command. First reactive
 * consumer of TanStack Query in this app — pattern to copy for further list
 * queries (`useRuns`, `useModels`, etc.).
 *
 * Cache invalidation: import flows that mutate `datasets` should call
 * `queryClient.invalidateQueries({ queryKey: ["datasets"] })` after success.
 *
 * Polling: while any dataset is `importing`, refetch every 500 ms so the
 * row-count column tick is visible without a manual refresh. Stops polling
 * once every dataset is in a terminal state (`ready` / `failed`).
 */
export function useDatasets() {
  return useQuery({
    queryKey: ["datasets"],
    queryFn: async (): Promise<DatasetSummary[]> => {
      const result = await commands.listDatasets();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: (query) => {
      const data = query.state.data as DatasetSummary[] | undefined;
      if (!data) return false;
      // 1 s keeps the progress meter live without piling IPC calls against a
      // DB that the Appender is hammering with bulk writes.
      return data.some((d) => d.importState === "importing") ? 1000 : false;
    },
  });
}

/**
 * The input profile the import worker stored on a dataset (profile.rs):
 * field shapes, checks, findings, and sample model inputs. `null` for
 * datasets imported before the profile existed. Written once at import
 * completion, so no polling; DatasetDetail invalidates it when an import
 * finishes.
 */
export function useInputProfile(datasetId: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ["inputProfile", toValue(datasetId)] as const),
    queryFn: async (): Promise<InputProfile | null> => {
      const result = await commands.getInputProfile(toValue(datasetId));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}
