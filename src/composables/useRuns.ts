import { type MaybeRefOrGetter, computed, toValue, watch } from "vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { type RunDetail, type RunSummary, commands } from "../bindings";

/**
 * Full runs list, server-sorted with active states first. The sidebar groups
 * the result by state for rendering. While any run is `running` the list
 * refetches every second — this is the app's global run heartbeat: it keeps
 * inactive surfaces (other dataset tabs' Classify disable, the sidebar)
 * honest even though only the active tab mounts a fast per-run poll.
 */
export function useRuns() {
  return useQuery({
    queryKey: ["runs"],
    queryFn: async (): Promise<RunSummary[]> => {
      const result = await commands.listRuns();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: (query) => {
      const data = query.state.data as RunSummary[] | undefined;
      return data?.some((r) => r.state === "running") ? 1000 : false;
    },
  });
}

/**
 * Most recent run for a dataset (or null if never classified). This is the
 * dataset tab's run surface card — backend state, not component memory, so
 * closing/reopening the tab or restarting the app rehydrates it. Polls at
 * 250 ms while that run is `running`, same cadence as `useRun`.
 */
export function useLatestRun(datasetId: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ["runs", "latest", toValue(datasetId)] as const),
    queryFn: async (): Promise<RunDetail | null> => {
      const result = await commands.getLatestRun(toValue(datasetId));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    refetchInterval: (query) => {
      const data = query.state.data as RunDetail | null | undefined;
      return data?.state === "running" ? 250 : false;
    },
  });
}

/**
 * Global run-lifecycle watcher. Mounted once (App.vue). Rides the `useRuns`
 * heartbeat: when any run leaves `running`, invalidate every surface a
 * finished run can change — courses (new classification columns), coverage
 * chips, dataset metadata, dashboard metrics, and the run queries themselves.
 * Centralizing this here (instead of per-tab watchers) is what makes
 * completion refresh work when the owning tab isn't mounted.
 */
export function useRunLifecycleRefresh() {
  const queryClient = useQueryClient();
  const { data: runs } = useRuns();
  const lastStates = new Map<string, string>();
  watch(runs, (list) => {
    if (!list) return;
    let anyFinished = false;
    for (const run of list) {
      const prev = lastStates.get(run.id);
      if (prev === "running" && run.state !== "running") anyFinished = true;
      lastStates.set(run.id, run.state);
    }
    if (anyFinished) {
      queryClient.invalidateQueries({ queryKey: ["courses"] });
      queryClient.invalidateQueries({ queryKey: ["coverage"] });
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
      queryClient.invalidateQueries({ queryKey: ["runs"] });
    }
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
