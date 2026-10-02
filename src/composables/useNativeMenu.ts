import { onBeforeUnmount, onMounted } from "vue";
import { events, type MenuAction } from "../bindings";
import type { ActivityId } from "../config/activities";
import type { SettingsSectionId } from "../config/settingsSections";
import { useWorkspace } from "../stores/workspace";
import { usePauseRun, useRuns } from "./useRuns";

/**
 * One handler per menu command, each a direct call to the action the in-app
 * UI already uses. `Record<MenuAction, …>` makes vue-tsc fail if `menu.rs`
 * gains an action without a handler here, or if a handler outlives its
 * action. The Windows/Linux custom titlebar menu (#104) should dispatch
 * through this same table.
 */
export function useMenuActions(): Record<MenuAction, () => void> {
  const workspace = useWorkspace();
  const toast = useToast();
  const { data: runs } = useRuns();
  const pauseRun = usePauseRun();

  // Same steps as CommandPalette's jumpToActivity / jumpToSettingsSection.
  function show(activity: ActivityId, section?: SettingsSectionId): void {
    workspace.setActiveActivity(activity);
    if (section) workspace.setActiveSettingsSection(section);
    if (!workspace.sidebarOpen) workspace.toggleSidebar();
  }

  // DatasetDetail's Classify / Export buttons, for the selected dataset.
  function datasetAction(action: "classify" | "export"): void {
    show("datasets");
    if (workspace.selectedDatasetId === null) {
      toast.add({ title: "Select a dataset first", color: "neutral" });
      return;
    }
    workspace.requestDatasetAction(action);
  }

  return {
    about: () => show("settings", "about"),
    preferences: () => show("settings"),
    // DatasetsSidebar's "Import CSV" button.
    import_csv: () => workspace.openImportDialog(),
    export_results: () => datasetAction("export"),
    start_classification: () => datasetAction("classify"),
    // RunDetail / DatasetDetail's Pause button, aimed at the one running run
    // (only one may be active app-wide).
    pause_run: () => {
      const active = runs.value?.find((run) => run.state === "running");
      if (!active) {
        toast.add({ title: "No run is in progress", color: "neutral" });
        return;
      }
      pauseRun.mutate(active.id);
    },
    toggle_sidebar: () => workspace.toggleSidebar(),
    toggle_command_palette: () => workspace.toggleCommandPalette(),
  };
}

/**
 * Bridge macOS native menu clicks and accelerators (`menu.rs`) to
 * `useMenuActions`. Call once, in `Workbench.vue`, which mounts only once
 * startup is ready (#224): until then no menu action runs.
 */
export function useNativeMenu(): void {
  const actions = useMenuActions();
  let unlisten: (() => void) | undefined;
  onMounted(async () => {
    unlisten = await events.menuActionTriggered.listen(({ payload }) => {
      actions[payload.action]();
    });
  });
  onBeforeUnmount(() => {
    unlisten?.();
  });
}
