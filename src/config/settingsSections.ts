import type { Component } from "vue";
import AboutSettings from "../views/settings/AboutSettings.vue";
import GeneralSettings from "../views/settings/GeneralSettings.vue";
import InferenceSettings from "../views/settings/InferenceSettings.vue";
import ThemeSettings from "../views/settings/ThemeSettings.vue";

/** Stable id for each Settings sub-section. Persisted in the workspace store. */
export type SettingsSectionId = "general" | "inference" | "theme" | "about";

export interface SettingsSectionDef {
  id: SettingsSectionId;
  label: string;
  icon: string;
  component: Component;
}

/**
 * Settings sub-section registry, in sidebar render order. Add a row to introduce
 * a new section; the sidebar and panel pick it up automatically.
 */
export const settingsSections: SettingsSectionDef[] = [
  {
    id: "general",
    label: "General",
    icon: "i-lucide-settings-2",
    component: GeneralSettings,
  },
  {
    id: "inference",
    label: "Compute",
    icon: "i-lucide-cpu",
    component: InferenceSettings,
  },
  {
    id: "theme",
    label: "Appearance",
    icon: "i-lucide-palette",
    component: ThemeSettings,
  },
  {
    id: "about",
    label: "About",
    icon: "i-lucide-info",
    component: AboutSettings,
  },
];

export function sectionById(id: SettingsSectionId): SettingsSectionDef | undefined {
  return settingsSections.find((section) => section.id === id);
}
