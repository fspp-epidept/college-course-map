import { useQuery } from "@tanstack/vue-query";
import { type DatasetSummary, commands } from "../bindings";

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
      return data.some((d) => d.importState === "importing") ? 500 : false;
    },
  });
}
