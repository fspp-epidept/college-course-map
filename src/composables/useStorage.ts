import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { type ClearTarget, type PruneScope, type StorageStatus, commands } from "../bindings";

/**
 * What the app keeps on disk (Settings → Storage). Measured in Rust on each
 * fetch; refetched when the section mounts and after anything that changes
 * it (deletes, prunes, clears).
 */
export function useStorageStatus() {
  return useQuery({
    queryKey: ["storage"],
    queryFn: async (): Promise<StorageStatus> => {
      const result = await commands.storageStatus();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

/**
 * Remove cached classifications nothing uses. Resolves to the number
 * removed. The space comes back when the database is next compacted.
 */
export function usePruneCache() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (scope: PruneScope): Promise<number> => {
      const result = await commands.storagePrune(scope);
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["storage"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
    },
  });
}

/**
 * Compact the database. On success the app relaunches (the compacted copy
 * is put in place at the next start), so there is nothing to refresh.
 */
export function useCompactDatabase() {
  return useMutation({
    mutationFn: async () => {
      const result = await commands.compactDatabase();
      if (result.status === "error") throw new Error(result.error);
    },
  });
}

/** Delete the leftover files of one kind. */
export function useClearStorage() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (target: ClearTarget) => {
      const result = await commands.storageClear(target);
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["storage"] });
    },
  });
}

/** A byte count for display: `812 KB`, `43 MB`, `1.8 GB`. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${Math.round(bytes / 1_000_000)} MB`;
  if (bytes >= 1_000) return `${Math.round(bytes / 1_000)} KB`;
  return `${bytes} B`;
}
