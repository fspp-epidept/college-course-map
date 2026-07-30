import type { Component } from "vue";
import DatasetsPanel from "../views/datasets/DatasetsPanel.vue";
import DatasetsSidebar from "../views/datasets/DatasetsSidebar.vue";
import ModelsPanel from "../views/models/ModelsPanel.vue";
import ModelsSidebar from "../views/models/ModelsSidebar.vue";
import OverviewPanel from "../views/overview/OverviewPanel.vue";
import OverviewSidebar from "../views/overview/OverviewSidebar.vue";
import RunsPanel from "../views/runs/RunsPanel.vue";
import RunsSidebar from "../views/runs/RunsSidebar.vue";
import SettingsPanel from "../views/settings/SettingsPanel.vue";
import SettingsSidebar from "../views/settings/SettingsSidebar.vue";

/** Stable id for each top-level "activity" (entry in the activity bar). */
export type ActivityId = "overview" | "datasets" | "runs" | "models" | "settings";

export interface ActivityDef {
  id: ActivityId;
  label: string;
  icon: string;
  /** Component rendered inside the primary sidebar when this activity is active. */
  sidebar: Component;
  /** The component rendered as the main panel. Master/detail activities
   *  (Datasets, Runs) render their sidebar selection's detail here (EPI-58). */
  panel: Component;
  /** Anchor this activity at the bottom of the activity bar (Settings). */
  pinToBottom?: boolean;
}

/**
 * The activity bar's contents, in render order. Add an entry to introduce a
 * new top-level section.
 */
export const activities: ActivityDef[] = [
  {
    id: "overview",
    label: "Overview",
    icon: "i-lucide-house",
    sidebar: OverviewSidebar,
    panel: OverviewPanel,
  },
  {
    id: "datasets",
    label: "Datasets",
    icon: "i-lucide-database",
    sidebar: DatasetsSidebar,
    panel: DatasetsPanel,
  },
  {
    id: "runs",
    label: "Runs",
    icon: "i-lucide-play",
    sidebar: RunsSidebar,
    panel: RunsPanel,
  },
  {
    id: "models",
    label: "Models",
    icon: "i-lucide-cpu",
    sidebar: ModelsSidebar,
    panel: ModelsPanel,
  },
  {
    id: "settings",
    label: "Settings",
    icon: "i-lucide-settings",
    sidebar: SettingsSidebar,
    panel: SettingsPanel,
    pinToBottom: true,
  },
];

export function activityById(id: ActivityId): ActivityDef | undefined {
  return activities.find((activity) => activity.id === id);
}
