import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { onMounted, onUnmounted, ref } from "vue";
import { type EpKind, type RuntimeStatus, type Settings, commands, events } from "../bindings";
import { patchSettings } from "./useSettings";

/**
 * Runtime pack + execution provider state (EPI-73): which ONNX Runtime pack
 * this process loaded, which packs are installed, and which EP the loaded
 * models resolved to. `runtimeStateChanged` invalidates on pack installs;
 * `modelsStateChanged` covers resolved-EP changes after a reload.
 */
export function useRuntimeStatus() {
  return useQuery({
    queryKey: ["runtime"],
    queryFn: async (): Promise<RuntimeStatus> => {
      const result = await commands.runtimeStatus();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

/** The persisted EP priority list (settings.json). */
export function useEpPriority() {
  return useQuery({
    queryKey: ["settings", "executionProviders"],
    queryFn: async (): Promise<EpKind[]> => {
      const result = await commands.readSettings();
      if (result.status === "error") throw new Error(result.error);
      // The field is optional in the bindings (serde default fills it on the
      // Rust side); a missing value means "platform default order".
      return (result.data as Required<Settings>).executionProviders;
    },
  });
}

/**
 * Persist a reordered EP priority list, then reload models so sessions
 * re-register providers — the reorder is live once the reload finishes
 * (models_status.loading covers the interim).
 */
export function useSetEpPriority() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (executionProviders: EpKind[]) => {
      await patchSettings({ executionProviders });
      const reload = await commands.reloadModels();
      if (reload.status === "error") throw new Error(reload.error);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["settings"] });
      queryClient.invalidateQueries({ queryKey: ["runtime"] });
      queryClient.invalidateQueries({ queryKey: ["models"] });
    },
  });
}

export interface RuntimeDownload {
  received: number;
  total: number;
  bytesPerSec: number;
}

/**
 * Subscribe to runtime pack lifecycle events for the lifetime of the calling
 * component. Returns live download progress keyed by pack id.
 */
export function useRuntimeEvents() {
  const queryClient = useQueryClient();
  const progress = ref<Record<string, RuntimeDownload>>({});
  const unlisteners: Array<() => void> = [];

  onMounted(async () => {
    unlisteners.push(
      await events.runtimeStateChanged.listen(() => {
        queryClient.invalidateQueries({ queryKey: ["runtime"] });
      }),
    );
    unlisteners.push(
      await events.runtimeDownloadProgress.listen(({ payload }) => {
        progress.value = {
          ...progress.value,
          [payload.packId]: {
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

export function useDownloadRuntime() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (packId: string) => {
      const result = await commands.downloadRuntime(packId);
      if (result.status === "error") throw new Error(result.error);
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: ["runtime"] });
    },
  });
}
