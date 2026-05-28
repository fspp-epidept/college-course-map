import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { ActivityId, TabKind } from "../config/activities";

/**
 * One open thing in a tabbed activity (a dataset detail, a run detail, etc.).
 * `id` is unique within an activity — typically `<kind>:<resource-id>` so the
 * same dataset can't be opened twice. `kind` selects the body component from
 * the activities-config tab-kind registry.
 */
export interface OpenTab {
  id: string;
  kind: TabKind;
  label: string;
  icon?: string;
}

/**
 * Workspace state — the VS-Code-style "what is currently open" model. Tabs are
 * kept per-activity so switching from Datasets to Settings and back restores
 * exactly what you had open (the modality the demo design called for).
 */
export const useWorkspace = defineStore(
  "workspace",
  () => {
    const activeActivityId = ref<ActivityId>("overview");

    // Map<activityId, OpenTab[]>. Plain Record so persistedstate can serialize it.
    const tabsByActivity = ref<Partial<Record<ActivityId, OpenTab[]>>>({});
    const activeTabIdByActivity = ref<Partial<Record<ActivityId, string>>>({});

    // Primary sidebar visibility. Per-session for now (not persisted) so a
    // demo opens with the sidebar showing every time.
    const sidebarOpen = ref(true);

    function setActiveActivity(id: ActivityId): void {
      activeActivityId.value = id;
    }

    function openTab(activityId: ActivityId, tab: OpenTab): void {
      const list = tabsByActivity.value[activityId] ?? [];
      if (!list.some((existing) => existing.id === tab.id)) {
        tabsByActivity.value[activityId] = [...list, tab];
      }
      activeTabIdByActivity.value[activityId] = tab.id;
    }

    function closeTab(activityId: ActivityId, tabId: string): void {
      const list = tabsByActivity.value[activityId] ?? [];
      const index = list.findIndex((tab) => tab.id === tabId);
      if (index === -1) return;
      const next = list.filter((tab) => tab.id !== tabId);
      tabsByActivity.value[activityId] = next;
      // If we just closed the active tab, focus a neighbor (or clear).
      if (activeTabIdByActivity.value[activityId] === tabId) {
        const neighbor = next[Math.min(index, next.length - 1)];
        if (neighbor) {
          activeTabIdByActivity.value[activityId] = neighbor.id;
        } else {
          delete activeTabIdByActivity.value[activityId];
        }
      }
    }

    function setActiveTab(activityId: ActivityId, tabId: string): void {
      activeTabIdByActivity.value[activityId] = tabId;
    }

    function toggleSidebar(): void {
      sidebarOpen.value = !sidebarOpen.value;
    }

    // Convenience views over the active activity.
    const activeTabs = computed<OpenTab[]>(
      () => tabsByActivity.value[activeActivityId.value] ?? [],
    );
    const activeTabId = computed<string | undefined>(
      () => activeTabIdByActivity.value[activeActivityId.value],
    );
    const activeTab = computed<OpenTab | undefined>(() =>
      activeTabs.value.find((tab) => tab.id === activeTabId.value),
    );

    return {
      activeActivityId,
      tabsByActivity,
      activeTabIdByActivity,
      sidebarOpen,
      activeTabs,
      activeTabId,
      activeTab,
      setActiveActivity,
      openTab,
      closeTab,
      setActiveTab,
      toggleSidebar,
    };
  },
  {
    // Persist only what should restore across reloads. sidebarOpen is per-session.
    persist: {
      pick: ["activeActivityId", "tabsByActivity", "activeTabIdByActivity"],
    },
  },
);
