<script setup lang="ts">
import { computed } from "vue";
import { sectionById, settingsSections } from "../../config/settingsSections";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Resolve the active section's component. Falls back to General if the persisted
// id ever points at a section we removed — graceful, not load-bearing.
const activeSection = computed(
  () => sectionById(workspace.activeSettingsSection) ?? settingsSections[0],
);
</script>

<template>
  <div class="h-full overflow-y-auto px-8 py-6">
    <component :is="activeSection?.component" />
  </div>
</template>
