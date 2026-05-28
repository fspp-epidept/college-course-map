<script setup lang="ts">
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Stub run list grouped by state — mirrors what the real list will look like.
const stubGroups = [
  {
    label: "Active",
    runs: [{ id: "run-0042", name: "panel.csv · 6-digit", note: "running · 42%" }],
  },
  {
    label: "Completed",
    runs: [
      { id: "run-0041", name: "panel.csv · 4-digit", note: "yesterday" },
      { id: "run-0040", name: "sample-courses · 2-digit", note: "2 days ago" },
    ],
  },
];

function open(run: { id: string; name: string }): void {
  workspace.openTab("runs", {
    id: `run:${run.id}`,
    kind: "run",
    label: run.id,
    icon: "i-lucide-play",
  });
}
</script>

<template>
  <div class="flex flex-col gap-3 p-2">
    <section v-for="group in stubGroups" :key="group.label" class="flex flex-col gap-1">
      <p class="px-2 text-xs uppercase tracking-wide text-(--ui-text-dimmed)">
        {{ group.label }}
      </p>
      <button
        v-for="run in group.runs"
        :key="run.id"
        type="button"
        class="text-left rounded px-2 py-1.5 hover:bg-(--ui-bg-muted) flex flex-col"
        @click="open(run)"
      >
        <span class="text-sm text-(--ui-text)">{{ run.name }}</span>
        <span class="text-xs text-(--ui-text-dimmed)">{{ run.note }}</span>
      </button>
    </section>
  </div>
</template>
