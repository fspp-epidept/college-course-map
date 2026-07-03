<script setup lang="ts">
import { computed, watch } from "vue";
import type { RunSummary } from "../../bindings";
import { useRuns } from "../../composables/useRuns";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();
const { data: runs, isPending, isError, error } = useRuns();

// Same pattern as DatasetsSidebar: drop persisted tabs whose run no longer
// exists in the DB. Keeps stale tabs from sticking around after a DB wipe.
watch(
  runs,
  (list) => {
    if (!list) return;
    const valid = new Set(list.map((r) => `run:${r.id}`));
    const open = workspace.tabsByActivity.runs ?? [];
    for (const tab of open) {
      if (!valid.has(tab.id)) {
        workspace.closeTab("runs", tab.id);
      }
    }
  },
  { immediate: true },
);

interface RunGroup {
  label: string;
  states: ReadonlySet<string>;
  runs: RunSummary[];
}

// Group order matches the server-side ORDER BY so the sidebar reads top-down
// the way users skim it: what's running, then what's done, then anything
// off-happy-path.
const groups = computed<RunGroup[]>(() => {
  const all = runs.value ?? [];
  const buckets: { label: string; states: ReadonlySet<string> }[] = [
    { label: "Active", states: new Set(["running", "pending", "paused"]) },
    // Interrupted runs get their own group: they're the actionable ones
    // (resumable), and burying them under "Other" hides that (EPI-69).
    { label: "Paused", states: new Set(["interrupted"]) },
    { label: "Completed", states: new Set(["completed"]) },
    { label: "Other", states: new Set(["failed", "cancelled"]) },
  ];
  return buckets
    .map((b) => ({ ...b, runs: all.filter((r) => b.states.has(r.state)) }))
    .filter((b) => b.runs.length > 0);
});

function open(run: RunSummary): void {
  // Use the dataset title + a short id suffix for a meaningful tab label.
  const shortId = run.id.slice(0, 8);
  workspace.openTab("runs", {
    id: `run:${run.id}`,
    kind: "run",
    label: `${run.datasetTitle} · ${shortId}`,
    icon: "i-lucide-play",
  });
}

function progressLabel(run: RunSummary): string {
  const level = run.digitLevel !== null ? `${run.digitLevel}-digit · ` : "";
  if (
    run.state === "running" &&
    run.rowsTotal !== null &&
    run.rowsTotal !== undefined &&
    run.rowsProcessed !== null &&
    run.rowsProcessed !== undefined
  ) {
    return `${level}${run.rowsProcessed} / ${run.rowsTotal}`;
  }
  if (run.state === "completed" && run.rowsProcessed !== null && run.rowsProcessed !== undefined) {
    return `${level}${run.rowsProcessed} rows`;
  }
  if (run.state === "interrupted") {
    // Resumability is the actionable fact about a paused run — say it in the
    // row, not just the detail view (EPI-69).
    return `${level}${run.resumable ? "paused · resumable" : "paused"}`;
  }
  return `${level}${run.state}`;
}
</script>

<template>
  <div class="flex flex-col gap-3 p-2">
    <p v-if="isPending" class="px-2 py-1.5 text-sm text-(--ui-text-dimmed)">
      Loading runs…
    </p>

    <p v-else-if="isError" class="px-2 py-1.5 text-sm text-(--ui-color-error-500)">
      Failed to load runs: {{ error?.message }}
    </p>

    <template v-else-if="groups.length > 0">
      <section v-for="group in groups" :key="group.label" class="flex flex-col gap-1">
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
          <span class="text-sm text-(--ui-text) truncate">{{ run.datasetTitle }}</span>
          <span class="text-xs text-(--ui-text-dimmed)">
            {{ progressLabel(run) }}
          </span>
        </button>
      </section>
    </template>

    <p v-else class="px-2 py-1.5 text-sm text-(--ui-text-dimmed)">
      No runs yet. Open a dataset and click Classify.
    </p>
  </div>
</template>
