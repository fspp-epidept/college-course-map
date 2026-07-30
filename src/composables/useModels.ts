import { type QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { type ModelStatus, commands, events } from "../bindings";

/**
 * Manifest-driven model status (EPI-56). The backend emits
 * `modelsStateChanged` whenever a download finishes a file or a load
 * starts/completes — the singleton listener below turns those into
 * invalidations, so components render fresh status without polling.
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

// Module-level singleton (EPI-74, same pattern as useTheme): download
// progress and its event subscriptions live for the app's lifetime, not any
// component's. Switching activities unmounts panels, but the listener and
// the progress it accumulated survive; a remount reads the same ref. The
// backend remains the source of truth for *whether* a download is running
// (`models_status().downloading`) — this ref only carries live positions.
const progress = ref<Record<number, DownloadProgress>>({});
let listenersStarted = false;

function ensureListeners(queryClient: QueryClient): void {
  if (listenersStarted) return;
  listenersStarted = true;
  void events.modelsStateChanged.listen(() => {
    queryClient.invalidateQueries({ queryKey: ["models"] });
  });
  void events.modelDownloadProgress.listen(({ payload }) => {
    progress.value = {
      ...progress.value,
      [payload.digitLevel]: {
        file: payload.file,
        received: payload.received,
        total: payload.total,
        bytesPerSec: payload.bytesPerSec,
      },
    };
  });
}

/**
 * Subscribe (once, app-wide) to model lifecycle events and expose live
 * per-model download progress keyed by digit level, hydrated from the
 * backend's `models_status` snapshots for positions from before this
 * client mounted.
 */
export function useModelsEvents() {
  ensureListeners(useQueryClient());
  return { progress };
}

/**
 * Whether a download is in flight app-wide, from the backend's own guard —
 * survives any component unmount and reflects downloads this client never
 * initiated. This is what gates Download buttons (EPI-74).
 */
export function useDownloading() {
  const { data } = useModelsStatus();
  return computed(() => data.value?.some((m) => m.downloading) ?? false);
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

export function useCancelDownload() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      await commands.cancelDownload();
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
