<script setup lang="ts">
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Placeholder rows until #112 wires real dataset loading. Click opens a tab.
const stubDatasets = [
  { id: "panel.csv", name: "panel.csv", note: "165 MB · 14 cols" },
  { id: "sample-courses.csv", name: "sample-courses.csv", note: "8 KB · 14 cols" },
  { id: "validation.csv", name: "validation.csv", note: "imported 2026-05-10" },
];

function open(dataset: { id: string; name: string }): void {
  workspace.openTab("datasets", {
    id: `dataset:${dataset.id}`,
    kind: "dataset",
    label: dataset.name,
    icon: "i-lucide-database",
  });
}
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <button
      v-for="dataset in stubDatasets"
      :key="dataset.id"
      type="button"
      class="text-left rounded px-2 py-1.5 hover:bg-(--ui-bg-muted) flex flex-col"
      @click="open(dataset)"
    >
      <span class="text-sm text-(--ui-text)">{{ dataset.name }}</span>
      <span class="text-xs text-(--ui-text-dimmed)">{{ dataset.note }}</span>
    </button>

    <button
      type="button"
      class="mt-2 text-left rounded px-2 py-1.5 text-sm text-(--ui-text-muted) hover:bg-(--ui-bg-muted) flex items-center gap-2"
    >
      <UIcon name="i-lucide-plus" class="size-4" />
      <span>Import CSV</span>
    </button>
  </div>
</template>
