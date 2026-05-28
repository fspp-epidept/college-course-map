import type { Component } from "vue";
import DatasetsSidebar from "../views/datasets/DatasetsSidebar.vue";
import DatasetTabPanel from "../views/datasets/DatasetTabPanel.vue";
import ModelsPanel from "../views/models/ModelsPanel.vue";
import ModelsSidebar from "../views/models/ModelsSidebar.vue";
import OverviewPanel from "../views/overview/OverviewPanel.vue";
import OverviewSidebar from "../views/overview/OverviewSidebar.vue";
import RunsSidebar from "../views/runs/RunsSidebar.vue";
import RunTabPanel from "../views/runs/RunTabPanel.vue";
import SettingsPanel from "../views/settings/SettingsPanel.vue";
import SettingsSidebar from "../views/settings/SettingsSidebar.vue";

/** Stable id for each top-level "activity" (entry in the activity bar). */
export type ActivityId = "overview" | "datasets" | "runs" | "models" | "settings";

/** "Tabbed" activities open multiple things at once (Datasets, Runs);
 *  "fixed" activities show one panel (Overview, Models, Settings). */
export type ActivityKind = "tabbed" | "fixed";

/** Kinds of openable tab body. Add a new kind here when a new tabbable
 *  resource appears, and register its component in `tabKindPanels` below. */
export type TabKind = "dataset" | "run";

export interface ActivityDef {
  id: ActivityId;
  label: string;
  icon: string;
  kind: ActivityKind;
  /** Component rendered inside the primary sidebar when this activity is active. */
  sidebar: Component;
  /** Required for `kind: "fixed"`: the component rendered as the main panel. */
  panel?: Component;
  /** Anchor this activity at the bottom of the activity bar (Settings). */
  pinToBottom?: boolean;
}

/**
 * Tab-body component registry, keyed by `TabKind`. The workbench's TabbedView
 * looks up the component by tab.kind when rendering the active tab's body, so
 * tab content stays decoupled from the activity definition.
 */
export const tabKindPanels: Record<TabKind, Component> = {
  dataset: DatasetTabPanel,
  run: RunTabPanel,
};

/**
 * The activity bar's contents, in render order. Add an entry to introduce a
 * new top-level section.
 */
export const activities: ActivityDef[] = [
  {
    id: "overview",
    label: "Overview",
    icon: "i-lucide-house",
    kind: "fixed",
    sidebar: OverviewSidebar,
    panel: OverviewPanel,
  },
  {
    id: "datasets",
    label: "Datasets",
    icon: "i-lucide-database",
    kind: "tabbed",
    sidebar: DatasetsSidebar,
  },
  {
    id: "runs",
    label: "Runs",
    icon: "i-lucide-play",
    kind: "tabbed",
    sidebar: RunsSidebar,
  },
  {
    id: "models",
    label: "Models",
    icon: "i-lucide-cpu",
    kind: "fixed",
    sidebar: ModelsSidebar,
    panel: ModelsPanel,
  },
  {
    id: "settings",
    label: "Settings",
    icon: "i-lucide-settings",
    kind: "fixed",
    sidebar: SettingsSidebar,
    panel: SettingsPanel,
    pinToBottom: true,
  },
];

export function activityById(id: ActivityId): ActivityDef | undefined {
  return activities.find((activity) => activity.id === id);
}
