import { type MaybeRefOrGetter, computed, toValue } from "vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { type RunDetail, type RunSummary, commands } from "../bindings";

/**
 * Full runs list, server-sorted with active states first. The sidebar groups
 * the result by state for rendering.
 */
export function useRuns() {
  return useQuery({
    queryKey: ["runs"],
    queryFn: async (): Promise<RunSummary[]> => {
      const result = await commands.listRuns();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

/**
 * One run by id. While the run is `running`, refetches every 250 ms so the
 * progress meter ticks. Polling stops automatically once the run reaches a
 * terminal state.
 */
export function useRun(id: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ["runs", toValue(id)] as const),
    queryFn: async (): Promise<RunDetail> => {
      const result = await commands.getRun(toValue(id));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: (query) => {
      const data = query.state.data as RunDetail | undefined;
      return data?.state === "running" ? 250 : false;
    },
  });
}

/**
 * Request a graceful pause of a running run. The worker stops at its next batch
 * boundary and finalizes as `interrupted`; the `useRun` poll picks up the new
 * state on its next tick. Invalidates the run + runs-list queries so any
 * non-polling view (the sidebar) also refreshes.
 */
export function usePauseRun() {
  const queryClient = useQueryClient();
  return useMutation({
    // pause_run can't fail (it just flips a flag), so the binding returns a
    // bare boolean rather than the usual Result envelope.
    mutationFn: (runId: string): Promise<boolean> => commands.pauseRun(runId),
    onSuccess: (_signalled, runId) => {
      queryClient.invalidateQueries({ queryKey: ["runs", runId] });
      queryClient.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}
