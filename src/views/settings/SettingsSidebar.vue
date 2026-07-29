<script setup lang="ts">
import { type SettingsSectionId, settingsSections } from "../../config/settingsSections";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// The panel is one scrolling page (EPI-95); a sidebar entry scrolls to its
// section anchor rather than swapping panels. Direct-in-handler so clicking
// the already-active entry still scrolls.
function goTo(id: SettingsSectionId): void {
  workspace.setActiveSettingsSection(id);
  document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
}
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <p class="px-2 text-xs uppercase tracking-wide text-(--ui-text-dimmed)">
      Sections
    </p>
    <button
      v-for="section in settingsSections"
      :key="section.id"
      type="button"
      class="text-left rounded px-2 py-1.5 text-sm flex items-center gap-2"
      :class="
        section.id === workspace.activeSettingsSection
          ? 'bg-(--ui-bg-muted) text-(--ui-text)'
          : 'text-(--ui-text-muted) hover:bg-(--ui-bg-muted)'
      "
      @click="goTo(section.id)"
    >
      <UIcon :name="section.icon" class="size-4" />
      <span>{{ section.label }}</span>
    </button>
  </div>
</template>
