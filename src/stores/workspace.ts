import { acceptHMRUpdate, defineStore } from "pinia";
import { ref } from "vue";
import type { ActivityId } from "../config/activities";
import type { SettingsSectionId } from "../config/settingsSections";

// Primary sidebar width is in rem so it respects the user's font-size scale.
// Bounds keep the sidebar usable: too narrow and labels truncate to nothing;
// too wide and the main panel disappears.
export const SIDEBAR_WIDTH_MIN_REM = 12;
export const SIDEBAR_WIDTH_MAX_REM = 36;
const SIDEBAR_WIDTH_DEFAULT_REM = 16;

/**
 * Workspace state — the sidebar-driven master/detail model (EPI-58, replacing
 * the earlier VS-Code-style tabs): each activity shows one panel; Datasets and
 * Runs render the detail for the sidebar's current selection. Selection is
 * remembered per activity, so switching to Settings and back restores what
 * you were looking at.
 */
export const useWorkspace = defineStore(
  "workspace",
  () => {
    const activeActivityId = ref<ActivityId>("overview");

    // Master/detail selections (EPI-58). Null renders the activity's empty
    // state. Ids are backend resource ids — stale ones are pruned by the
    // sidebars when the backing row disappears (e.g. after db:clear-data).
    const selectedDatasetId = ref<string | null>(null);
    const selectedRunId = ref<string | null>(null);
    // CCM Reference selection: a taxonomy code, 2- or 6-digit. The
    // taxonomy is static, so no pruning — the detail shows "not found" if a
    // persisted code ever disappears.
    const selectedCcmCode = ref<string | null>(null);

    // Primary sidebar visibility. Per-session for now (not persisted) so a
    // demo opens with the sidebar showing every time.
    const sidebarOpen = ref(true);
    const sidebarWidthRem = ref(SIDEBAR_WIDTH_DEFAULT_REM);

    // Command palette open state, lifted into the store so non-keyboard surfaces
    // (the titlebar search button) can toggle the same UDashboardSearch instance.
    const commandPaletteOpen = ref(false);

    // Import dialog open state, lifted so the File menu (useNativeMenu) and the
    // Datasets sidebar button open the same root-mounted ImportCsvDialog.
    const importDialogOpen = ref(false);

    // A Classify/Export request from the menu for the selected dataset.
    // DatasetDetail consumes it and runs the same function its button does.
    const pendingDatasetAction = ref<"classify" | "export" | null>(null);

    // Settings activity has sub-sections. We route between them via this store
    // rather than vue-router so workbench state stays in one place. New
    // sections: extend `SettingsSectionId` in config/settingsSections.ts.
    const activeSettingsSection = ref<SettingsSectionId>("general");

    function setActiveActivity(id: ActivityId): void {
      activeActivityId.value = id;
    }

    function setActiveSettingsSection(id: SettingsSectionId): void {
      activeSettingsSection.value = id;
    }

    function selectDataset(id: string | null): void {
      // A menu request targets the dataset selected when it was made.
      if (id !== selectedDatasetId.value) pendingDatasetAction.value = null;
      selectedDatasetId.value = id;
    }

    function selectRun(id: string | null): void {
      selectedRunId.value = id;
    }

    function selectCcmCode(code: string | null): void {
      selectedCcmCode.value = code;
    }

    function toggleSidebar(): void {
      sidebarOpen.value = !sidebarOpen.value;
    }

    function setSidebarWidth(rem: number): void {
      sidebarWidthRem.value = Math.max(SIDEBAR_WIDTH_MIN_REM, Math.min(SIDEBAR_WIDTH_MAX_REM, rem));
    }

    function toggleCommandPalette(): void {
      commandPaletteOpen.value = !commandPaletteOpen.value;
    }

    function openImportDialog(): void {
      importDialogOpen.value = true;
    }

    function requestDatasetAction(action: "classify" | "export"): void {
      pendingDatasetAction.value = action;
    }

    return {
      activeActivityId,
      selectedDatasetId,
      selectedRunId,
      selectedCcmCode,
      sidebarOpen,
      sidebarWidthRem,
      commandPaletteOpen,
      importDialogOpen,
      pendingDatasetAction,
      activeSettingsSection,
      setActiveActivity,
      setActiveSettingsSection,
      selectDataset,
      selectRun,
      selectCcmCode,
      toggleSidebar,
      setSidebarWidth,
      toggleCommandPalette,
      openImportDialog,
      requestDatasetAction,
    };
  },
  {
    // Persist what should survive reload. sidebarOpen, commandPaletteOpen,
    // importDialogOpen and pendingDatasetAction are per-session (a demo opens
    // with the sidebar showing and the palette closed). The storage key is versioned: "workspace" carried the tabbed
    // era's shape (tabsByActivity etc.) and is deliberately orphaned rather
    // than migrated (EPI-58).
    persist: {
      key: "workspace-v2",
      pick: [
        "activeActivityId",
        "selectedDatasetId",
        "selectedRunId",
        "selectedCcmCode",
        "sidebarWidthRem",
        "activeSettingsSection",
      ],
    },
  },
);

// Vite HMR: accept module updates so adding/changing actions doesn't leave
// components holding a stale store reference (the "is not a function" bite).
if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useWorkspace, import.meta.hot));
}
