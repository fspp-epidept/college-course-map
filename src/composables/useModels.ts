import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { onMounted, onUnmounted, ref } from "vue";
import { type ModelStatus, commands, events } from "../bindings";

/**
 * Manifest-driven model status (EPI-56). The backend emits
 * `modelsStateChanged` whenever a download finishes a file or a load
 * starts/completes — `useModelsEvents` turns those into invalidations, so
 * components render fresh status without polling.
 */
export function useModelsStatus() {
  return useQuery({
    queryKey: ["models"],
    queryFn: async (): Promise<ModelStatus[]> => {
      const result = await commands.modelsStatus();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

/** Per-digit-level download progress, fed by `modelDownloadProgress` events. */
export interface DownloadProgress {
  file: string;
  received: number;
  total: number;
  bytesPerSec: number;
}

/**
 * Subscribe to model lifecycle events for the lifetime of the calling
 * component. Returns live per-model download progress keyed by digit level.
 */
export function useModelsEvents() {
  const queryClient = useQueryClient();
  const progress = ref<Record<number, DownloadProgress>>({});
  const unlisteners: Array<() => void> = [];

  onMounted(async () => {
    unlisteners.push(
      await events.modelsStateChanged.listen(() => {
        queryClient.invalidateQueries({ queryKey: ["models"] });
      }),
    );
    unlisteners.push(
      await events.modelDownloadProgress.listen(({ payload }) => {
        progress.value = {
          ...progress.value,
          [payload.digitLevel]: {
            file: payload.file,
            received: payload.received,
            total: payload.total,
            bytesPerSec: payload.bytesPerSec,
          },
        };
      }),
    );
  });
  onUnmounted(() => {
    for (const unlisten of unlisteners) unlisten();
  });

  return { progress };
}

export function useDownloadModels() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const result = await commands.downloadModels();
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["models"] });
    },
  });
}

export function useLoadModels() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const result = await commands.loadModels();
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["models"] });
    },
  });
}
