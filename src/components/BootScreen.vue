<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import packageJson from "../../package.json";
import type { Phase } from "../bindings";
import { commands } from "../bindings";
import { useBoot } from "../composables/useBoot";
import ResetAppData from "../views/settings/ResetAppData.vue";

// The boot screen (#224): stands in for the workbench while startup runs,
// and holds the recovery actions when it fails. One title per phase, one
// line saying what the current step is doing, one progress bar.

/** How long one phase runs before the screen says it is taking a while. */
const SLOW_MS = 10_000;

// Record, not a list: vue-tsc fails until a new Rust phase has its title.
const PHASE_TITLES: Record<Phase, string> = {
  MigratingData: "Preparing app data",
  OpeningDatabase: "Opening the database",
  UpgradingSchema: "Updating the database",
  LoadingRuntime: "Starting the classifier",
};

const { state } = useBoot();
const toast = useToast();

const failed = computed(() =>
  state.value?.status.status === "failed" ? state.value.status : null,
);
const title = computed(() =>
  state.value?.phase ? PHASE_TITLES[state.value.phase] : "Starting up",
);

// Indeterminate (null) until the step reports a total.
const progressValue = computed(() =>
  state.value && state.value.total > 0 ? state.value.done : null,
);

function formatBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)} MB`;
  return `${(bytes / 1e3).toFixed(0)} KB`;
}

const progressText = computed(() => {
  if (!state.value || state.value.total === 0) return null;
  const { done, total } = state.value;
  const percent = Math.min(100, Math.floor((done / total) * 100));
  return `${percent}% · ${formatBytes(done)} of ${formatBytes(total)}`;
});

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

async function restart(): Promise<void> {
  await commands.relaunchApp();
}

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
  if (!failed.value) return;
  const report = [
    `Course Classifier ${packageJson.version} (${import.meta.env.TAURI_ENV_PLATFORM})`,
    `Failed while: ${title.value}`,
    `Error: ${failed.value.message}`,
    ...failed.value.notices.map((notice) => `Notice: ${notice}`),
    ...(failed.value.logDir ? [`Logs: ${failed.value.logDir}`] : []),
  ].join("\n");
  try {
    await navigator.clipboard.writeText(report);
    toast.add({ title: "Error copied", color: "neutral" });
  } catch {
    toast.add({ title: "Couldn't copy the error", color: "error" });
  }
}
</script>

<template>
  <div class="flex flex-1 min-h-0 items-center justify-center overflow-y-auto p-8">
    <div class="w-full max-w-lg rounded-lg border border-default bg-elevated p-6 shadow-sm">
      <div v-if="!failed" role="status" aria-live="polite" class="flex flex-col gap-3">
        <div class="flex items-center gap-3">
          <UIcon
            name="i-lucide-loader-circle"
            class="size-5 shrink-0 text-primary motion-safe:animate-spin"
            aria-hidden="true"
          />
          <h1 class="text-base font-semibold text-highlighted">{{ title }}</h1>
        </div>
        <p v-if="state?.detail" class="text-sm text-muted">{{ state.detail }}</p>
        <UProgress
          :model-value="progressValue"
          :max="state?.total || 100"
          size="sm"
          :aria-label="title"
        />
        <p v-if="progressText" class="text-xs tabular-nums text-muted">{{ progressText }}</p>
        <p v-if="slow" class="mt-2 text-sm text-muted">
          This is taking longer than usual. Large databases can take a few minutes.
        </p>
      </div>

      <template v-else>
        <div role="alert" class="flex flex-col gap-3">
          <div class="flex items-center gap-3">
            <UIcon name="i-lucide-circle-x" class="size-5 shrink-0 text-error" aria-hidden="true" />
            <h1 class="text-base font-semibold text-highlighted">
              Course Classifier couldn't start
            </h1>
          </div>
          <p class="text-sm text-muted">
            Something went wrong while {{ title.toLowerCase() }}.
          </p>
          <pre
            class="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-3 font-mono text-xs text-toned"
            >{{ failed.message }}</pre
          >
          <ul v-if="failed.notices.length" class="flex flex-col gap-1 text-sm text-muted">
            <li v-for="notice in failed.notices" :key="notice">{{ notice }}</li>
          </ul>
        </div>
        <div class="mt-4 flex flex-wrap gap-2">
          <UButton icon="i-lucide-rotate-ccw" @click="restart">Restart App</UButton>
          <UButton icon="i-lucide-folder-open" color="neutral" variant="outline" @click="openLogs">
            Open Logs Folder
          </UButton>
          <UButton icon="i-lucide-copy" color="neutral" variant="ghost" @click="copyError">
            Copy Error
          </UButton>
        </div>
        <USeparator class="my-5" />
        <ResetAppData />
      </template>
    </div>
  </div>
</template>
