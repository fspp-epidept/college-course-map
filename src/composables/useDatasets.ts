import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
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
 * Polling: while any dataset is `importing`, refetch every second so the
 * row-count column tick is visible without a manual refresh; likewise while
 * one is `deleting`, so every surface sees it go. Stops once every dataset is
 * at rest (`ready` / `failed` / `delete_incomplete`).
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
      return data.some((d) => d.importState === "importing" || d.importState === "deleting")
        ? 1000
        : false;
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

/**
 * Delete a dataset with its courses and runs (#199). Slow on a large dataset;
 * the datasets list reports it as `deleting` meanwhile. Cached
 * classifications are not touched. Per-dataset queries are removed, not
 * refetched: there is nothing left to fetch.
 */
export function useDeleteDataset() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (datasetId: string) => {
      const result = await commands.deleteDataset(datasetId);
      if (result.status === "error") throw new Error(result.error);
    },
    onMutate: () => {
      // Pick up the `deleting` state, which also starts the list's polling.
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
    },
    onSettled: (_data, _error, datasetId) => {
      for (const key of ["courses", "coverage", "inputProfile"]) {
        queryClient.removeQueries({ queryKey: [key, datasetId] });
      }
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
      queryClient.invalidateQueries({ queryKey: ["runs"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
      queryClient.invalidateQueries({ queryKey: ["storage"] });
    },
  });
}
