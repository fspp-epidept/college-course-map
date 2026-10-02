<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { Phase } from "../bindings";
import { commands } from "../bindings";
import { useBoot } from "../composables/useBoot";
import ResetAppData from "../views/settings/ResetAppData.vue";

// The boot screen (#224): stands in for the workbench while startup runs,
// and holds the recovery actions when it fails.

/** How long one phase runs before the screen says it is taking a while. */
const SLOW_MS = 10_000;

const PHASES: { phase: Phase; label: string }[] = [
  { phase: "MigratingData", label: "Moving app data to its new location" },
  { phase: "OpeningDatabase", label: "Opening the database" },
  { phase: "UpgradingSchema", label: "Updating the database" },
  { phase: "LoadingRuntime", label: "Starting the classifier" },
];

const { state } = useBoot();
const toast = useToast();

const status = computed(() => state.value?.status.status ?? "starting");
const failure = computed(() =>
  state.value?.status.status === "failed" ? state.value.status.message : null,
);
const current = computed(() => PHASES.findIndex((p) => p.phase === state.value?.phase));

type RowState = "done" | "active" | "failed" | "pending";

const rows = computed(() =>
  PHASES.map(({ phase, label }, index): { phase: Phase; label: string; state: RowState } => {
    let row: RowState = "pending";
    if (status.value === "ready" || index < current.value) row = "done";
    else if (index === current.value) row = failure.value === null ? "active" : "failed";
    return { phase, label, state: row };
  }),
);

const ICONS: Record<RowState, string> = {
  done: "i-lucide-circle-check",
  active: "i-lucide-loader-circle",
  failed: "i-lucide-circle-x",
  pending: "i-lucide-circle",
};

// Indeterminate (null) until the step reports a total.
const progressValue = computed(() =>
  state.value && state.value.total > 0 ? state.value.done : null,
);

function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  return `${(bytes / 1e3).toFixed(0)} KB`;
}

const progressText = computed(() =>
  state.value && state.value.total > 0
    ? `${formatBytes(state.value.done)} of ${formatBytes(state.value.total)}`
    : null,
);

// "Taking longer than usual" after SLOW_MS in one phase.
const slow = ref(false);
let slowTimer: ReturnType<typeof setTimeout> | undefined;
watch(
  () => state.value?.phase,
  () => {
    slow.value = false;
    clearTimeout(slowTimer);
    slowTimer = setTimeout(() => (slow.value = true), SLOW_MS);
  },
  { immediate: true },
);
onBeforeUnmount(() => clearTimeout(slowTimer));

const failedLabel = computed(() => PHASES[current.value]?.label ?? "Starting up");

async function openLogs(): Promise<void> {
  const result = await commands.openLogsDir();
  if (result.status === "error") {
    toast.add({
      title: "Couldn't open the logs folder",
      description: result.error,
      color: "error",
    });
  }
}

async function copyError(): Promise<void> {
  try {
    await navigator.clipboard.writeText(`${failedLabel.value}: ${failure.value ?? ""}`);
    toast.add({ title: "Error copied", color: "neutral" });
  } catch {
    toast.add({ title: "Couldn't copy the error", color: "error" });
  }
}
</script>

<template>
  <div class="flex flex-1 min-h-0 items-center justify-center overflow-y-auto p-8">
    <div class="w-full max-w-md rounded-lg border border-default bg-elevated p-6 shadow-sm">
      <h1 class="text-base font-semibold text-highlighted">
        {{ failure === null ? "Getting ready" : "Course Classifier couldn't start" }}
      </h1>
      <p class="mt-1 text-sm text-muted">
        {{
          failure === null
            ? "This usually takes a moment."
            : "Something went wrong during startup. The logs folder has the details."
        }}
      </p>

      <ol class="mt-6 flex flex-col gap-4">
        <li v-for="row in rows" :key="row.phase" class="flex gap-3">
          <UIcon
            :name="ICONS[row.state]"
            class="size-5 shrink-0"
            :class="{
              'text-success': row.state === 'done',
              'text-primary animate-spin': row.state === 'active',
              'text-error': row.state === 'failed',
              'text-dimmed': row.state === 'pending',
            }"
          />
          <div class="flex min-w-0 flex-1 flex-col gap-2">
            <span
              class="text-sm leading-5"
              :class="{
                'text-muted': row.state === 'done',
                'font-medium text-highlighted': row.state === 'active' || row.state === 'failed',
                'text-dimmed': row.state === 'pending',
              }"
            >
              {{ row.label }}
            </span>
            <template v-if="row.state === 'active'">
              <UProgress :model-value="progressValue" :max="state?.total || 100" size="sm" />
              <span v-if="progressText" class="text-xs tabular-nums text-muted">
                {{ progressText }}
              </span>
            </template>
          </div>
        </li>
      </ol>

      <p v-if="failure === null && slow" class="mt-6 text-sm text-muted">
        This is taking longer than usual. Large databases can take a few minutes.
      </p>

      <template v-if="failure !== null">
        <pre
          class="mt-6 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-xs text-toned"
          >{{ failure }}</pre
        >
        <div class="mt-4 flex flex-wrap gap-2">
          <UButton icon="i-lucide-folder-open" color="neutral" variant="outline" @click="openLogs">
            Open logs folder
          </UButton>
          <UButton icon="i-lucide-copy" color="neutral" variant="ghost" @click="copyError">
            Copy error
          </UButton>
        </div>
        <USeparator class="my-5" />
        <ResetAppData />
      </template>
    </div>
  </div>
</template>
