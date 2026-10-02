<script setup lang="ts">
import type { RunSummary } from "../../bindings";
import { runStateMeta } from "./runState";

// A dataset's runs before its latest one (#247). The latest run has the run
// card above this list; these are records only. None can be resumed from
// here: a new run reuses everything they cached.
defineProps<{ runs: RunSummary[] }>();
const emit = defineEmits<{ delete: [runId: string] }>();

function when(run: RunSummary): string {
  return new Date(run.completedAt ?? run.lastProgressAt ?? run.createdAt).toLocaleString();
}
</script>

<template>
  <div class="flex flex-col gap-1">
    <h3 class="text-xs uppercase tracking-wide text-(--ui-text-dimmed)">Earlier runs</h3>
    <ul class="flex flex-col gap-1 text-sm">
      <li
        v-for="run in runs"
        :key="run.id"
        class="flex items-center gap-3 rounded border border-(--ui-border) px-3 py-1.5"
      >
        <!-- State by icon shape and text, never color alone. -->
        <UIcon
          :name="runStateMeta(run.state).icon"
          class="size-4 shrink-0"
          :class="runStateMeta(run.state).iconClass"
        />
        <span class="text-(--ui-text)">{{ runStateMeta(run.state).label }}</span>
        <span class="text-(--ui-text-muted)">{{ when(run) }}</span>
        <span class="flex-1 text-xs text-(--ui-text-dimmed) tabular-nums">
          {{ (run.rowsProcessed ?? 0).toLocaleString() }} classifications
          <template v-if="run.superseded"> · older model version</template>
        </span>
        <UButton
          v-if="run.state !== 'running'"
          icon="i-lucide-trash-2"
          variant="ghost"
          color="neutral"
          size="xs"
          aria-label="Delete run"
          title="Delete Run…"
          @click="emit('delete', run.id)"
        />
      </li>
    </ul>
  </div>
</template>
