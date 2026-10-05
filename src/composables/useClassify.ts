import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { type ComputedRef, computed, type MaybeRefOrGetter, ref, toValue, watch } from "vue";
import { type ClassifyProgress, commands } from "../bindings";
import { useDatasets } from "./useDatasets";

/** Course x model results a job has covered, summed over its models. */
export function progressDone(progress: ClassifyProgress): number {
  return progress.levels.reduce((sum, level) => sum + level.done, 0);
}

/** Course x model results a job covers when it is done. */
export function progressTotal(progress: ClassifyProgress): number {
  return progress.levels.reduce((sum, level) => sum + level.total, 0);
}

/**
 * Classify a dataset with every model. Starting and resuming are the same
 * call: courses already in the cache are skipped. The datasets list picks up
 * the `running` state and starts its fast poll.
 */
export function useClassifyDataset() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (datasetId: string) => {
      const result = await commands.classifyDataset(datasetId);
      if (result.status === "error") throw new Error(result.error);
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
    },
  });
}

/**
 * Ask a dataset's job to stop. The worker finishes its current batch first;
 * the list reports `stopping` meanwhile and `stopped` once it has.
 */
export function useStopClassification() {
  const queryClient = useQueryClient();
  return useMutation({
    // stop_classification can't fail (it flips a flag), so the binding
    // returns a bare boolean rather than the usual Result envelope.
    mutationFn: (datasetId: string): Promise<boolean> => commands.stopClassification(datasetId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
    },
  });
}

/**
 * Live classifications/s for a running job, derived from the list polling
 * (EPI-93) — zero extra IPC. Keeps a short ring of `(time, done)` samples,
 * skipping polls where the counter hasn't moved (progress lands per
 * super-chunk), and reports the rate across the ring. `null` until two flush
 * boundaries have been observed, and whenever no job is running.
 */
export function useClassifyRate(
  progress: MaybeRefOrGetter<ClassifyProgress | null | undefined>,
): ComputedRef<number | null> {
  const samples = ref<Array<{ t: number; done: number }>>([]);
  watch(
    () => {
      const p = toValue(progress);
      return p ? progressDone(p) : null;
    },
    (done) => {
      if (done === null) {
        samples.value = [];
        return;
      }
      if (samples.value[samples.value.length - 1]?.done === done) return;
      samples.value = [...samples.value.slice(-7), { t: Date.now(), done }];
    },
  );
  return computed(() => {
    const first = samples.value[0];
    const last = samples.value[samples.value.length - 1];
    if (!first || !last || last.t <= first.t || last.done <= first.done) return null;
    return ((last.done - first.done) * 1000) / (last.t - first.t);
  });
}

/**
 * Mounted once (Workbench.vue). Rides the datasets list: when a dataset
 * leaves `running`, refresh what a finished job changes — courses (new
 * classification columns), coverage, and metrics. Living here rather than on
 * the dataset page is what makes the refresh happen when that page isn't
 * mounted.
 */
export function useClassifyLifecycleRefresh() {
  const queryClient = useQueryClient();
  const { data: datasets } = useDatasets();
  const lastStates = new Map<string, string>();
  watch(datasets, (list) => {
    if (!list) return;
    let anyFinished = false;
    for (const dataset of list) {
      const prev = lastStates.get(dataset.id);
      if (prev === "running" && dataset.classification.state !== "running") anyFinished = true;
      lastStates.set(dataset.id, dataset.classification.state);
    }
    if (anyFinished) {
      queryClient.invalidateQueries({ queryKey: ["courses"] });
      queryClient.invalidateQueries({ queryKey: ["coverage"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
    }
  });
}
