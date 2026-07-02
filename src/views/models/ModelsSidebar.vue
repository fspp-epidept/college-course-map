<script setup lang="ts">
import { useModelsStatus } from "../../composables/useModels";

const { data: models } = useModelsStatus();
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <div
      v-for="m in models ?? []"
      :key="m.digitLevel"
      class="rounded px-2 py-1.5 hover:bg-(--ui-bg-muted) flex flex-col"
    >
      <span class="text-sm text-(--ui-text)">{{ m.digitLevel }}-digit</span>
      <span class="text-xs text-(--ui-text-dimmed) truncate">{{ m.hfRepo }}</span>
    </div>
  </div>
</template>
