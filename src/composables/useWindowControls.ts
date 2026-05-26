import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onUnmounted, ref } from "vue";

/// Thin wrapper over the current Tauri window for the custom titlebar's
/// window controls. `isMaximized` stays in sync so the maximize/restore
/// button can reflect state.
export function useWindowControls() {
  const appWindow = getCurrentWindow();
  const isMaximized = ref(false);

  async function syncMaximized() {
    isMaximized.value = await appWindow.isMaximized();
  }

  let unlisten: (() => void) | undefined;

  onMounted(async () => {
    await syncMaximized();
    unlisten = await appWindow.onResized(() => {
      void syncMaximized();
    });
  });

  onUnmounted(() => unlisten?.());

  return {
    isMaximized,
    minimize: () => appWindow.minimize(),
    toggleMaximize: () => appWindow.toggleMaximize(),
    close: () => appWindow.close(),
  };
}
