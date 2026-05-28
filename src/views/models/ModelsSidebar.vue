<script setup lang="ts">
// Placeholder model list (real list will come from the models table once
// #25 lands).
const stubModels = [
  { id: "ccm-2", label: "2-digit", note: "robotastronaut/ccm-roberta-2" },
  { id: "ccm-4", label: "4-digit", note: "robotastronaut/ccm-roberta-4" },
  { id: "ccm-6", label: "6-digit", note: "robotastronaut/ccm-roberta-6" },
];
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <div
      v-for="model in stubModels"
      :key="model.id"
      class="rounded px-2 py-1.5 hover:bg-(--ui-bg-muted) flex flex-col"
    >
      <span class="text-sm text-(--ui-text)">{{ model.label }}</span>
      <span class="text-xs text-(--ui-text-dimmed) truncate">{{ model.note }}</span>
    </div>
  </div>
</template>
