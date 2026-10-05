<script setup lang="ts">
import { computed, ref } from "vue";
import { type ClearTarget, type PruneScope, commands } from "../../bindings";
import {
  formatBytes,
  useClearStorage,
  useCompactDatabase,
  usePruneCache,
  useStorageStatus,
} from "../../composables/useStorage";
import ResetAppData from "./ResetAppData.vue";

// Settings → Storage (#200, #201): what the app keeps on disk, and the
// actions that give space back. Deleting a dataset happens on that
// dataset; this page is for everything else.
const { data: status, isPending, isError, error } = useStorageStatus();

const openError = ref<string | null>(null);
async function openDataFolder(): Promise<void> {
  const result = await commands.openDataDir();
  openError.value = result.status === "error" ? result.error : null;
}

// --- Database ---
// Below this a compaction isn't worth a relaunch.
const COMPACT_WORTHWHILE_BYTES = 10_000_000;
const databaseBytes = computed(() =>
  status.value ? status.value.database.fileBytes + status.value.database.walBytes : 0,
);
const canCompact = computed(
  () => (status.value?.database.reclaimableBytes ?? 0) >= COMPACT_WORTHWHILE_BYTES,
);
const compact = useCompactDatabase();
const compactOpen = ref(false);

// --- Cached classifications ---
const PRUNE_COPY: Record<PruneScope, { title: string; body: string }> = {
  superseded_models: {
    title: "Remove results from earlier model versions?",
    body: "These classifications were computed by a model version this release no longer uses. The app cannot show or export them. Removing them cannot be undone.",
  },
  unreferenced: {
    title: "Remove results for courses no longer in any dataset?",
    body: "These classifications are for courses that no dataset contains any more. If the same courses are imported again, they will be classified from scratch. Removing them cannot be undone.",
  },
};
const prune = usePruneCache();
const pruneScope = ref<PruneScope | null>(null);
const pruneOpen = computed({
  get: () => pruneScope.value !== null,
  set: (open) => {
    if (!open) pruneScope.value = null;
  },
});
const pruned = ref<number | null>(null);
function askPrune(scope: PruneScope): void {
  prune.reset();
  pruneScope.value = scope;
}
function onPrune(): void {
  if (pruneScope.value === null) return;
  prune.mutate(pruneScope.value, {
    onSuccess: (removed) => {
      pruned.value = removed;
      pruneScope.value = null;
    },
  });
}

// --- Leftover files ---
const clear = useClearStorage();
const total = (files: { bytes: number }[]) => files.reduce((sum, file) => sum + file.bytes, 0);
function onClear(target: ClearTarget): void {
  clear.mutate(target);
}
function fmtDate(iso: string | null): string {
  return iso ? new Date(iso).toLocaleDateString() : "";
}
</script>

<template>
  <section class="flex flex-col gap-6">
    <header class="flex items-start gap-3">
      <div class="flex-1">
        <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">Storage</h2>
        <p class="mt-1 text-sm text-(--ui-text-muted)">
          What the app keeps on this computer, and how to get space back. To delete a
          dataset, open it and use its Delete button.
        </p>
      </div>
      <UButton size="xs" variant="outline" class="shrink-0" @click="openDataFolder">
        Open Data Folder
      </UButton>
    </header>
    <p v-if="openError" class="text-sm text-(--ui-color-error-500)">{{ openError }}</p>

    <p v-if="isPending" class="text-sm text-(--ui-text-dimmed)">Measuring…</p>
    <p v-else-if="isError" class="text-sm text-(--ui-color-error-500)">
      {{ error?.message }}
    </p>

    <template v-else-if="status">
      <UAlert
        v-if="status.busy"
        color="neutral"
        variant="subtle"
        icon="i-lucide-info"
        title="Cleanup is unavailable right now"
        :description="status.busy"
      />

      <ul class="flex flex-col gap-1 text-sm">
        <!-- Database -->
        <li class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3">
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">Database</div>
            <div class="text-xs text-(--ui-text-muted)">
              Datasets and cached classifications.
              <template v-if="canCompact">
                {{ formatBytes(status.database.reclaimableBytes) }} of it is free space that
                compacting returns to the disk.
              </template>
              <template v-else>There is no free space in it to reclaim.</template>
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">{{ formatBytes(databaseBytes) }}</span>
          <UButton
            size="xs"
            variant="soft"
            class="shrink-0"
            :disabled="!canCompact || status.busy !== null"
            @click="compactOpen = true"
          >
            Compact Database…
          </UButton>
        </li>

        <!-- Cached classifications: rows, not bytes — most of the database
             is indexes, which can't be attributed to a bucket. -->
        <li class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex flex-col gap-2">
          <div class="flex items-center gap-3">
            <div class="flex-1 min-w-0">
              <div class="text-(--ui-text) font-medium">Cached classifications</div>
              <div class="text-xs text-(--ui-text-muted)">
                Results are kept so the same course is never classified twice. They live
                in the database; removing some frees space inside it, which compacting
                then returns to the disk.
              </div>
            </div>
            <span class="tabular-nums text-(--ui-text)">
              {{ status.cache.total.toLocaleString() }} results
            </span>
          </div>
          <div v-if="status.cache.superseded > 0" class="flex items-center gap-3 pl-3">
            <span class="flex-1 text-(--ui-text-muted)">From earlier model versions</span>
            <span class="tabular-nums text-(--ui-text)">
              {{ status.cache.superseded.toLocaleString() }}
            </span>
            <UButton
              size="xs"
              variant="soft"
              class="shrink-0"
              :disabled="status.busy !== null"
              @click="askPrune('superseded_models')"
            >
              Remove…
            </UButton>
          </div>
          <div v-if="status.cache.unreferenced > 0" class="flex items-center gap-3 pl-3">
            <span class="flex-1 text-(--ui-text-muted)">
              For courses no longer in any dataset
            </span>
            <span class="tabular-nums text-(--ui-text)">
              {{ status.cache.unreferenced.toLocaleString() }}
            </span>
            <UButton
              size="xs"
              variant="soft"
              class="shrink-0"
              :disabled="status.busy !== null"
              @click="askPrune('unreferenced')"
            >
              Remove…
            </UButton>
          </div>
          <p v-if="pruned !== null" class="pl-3 text-xs text-(--ui-text-muted)">
            Removed {{ pruned.toLocaleString() }}
            {{ pruned === 1 ? "result" : "results" }}. Compact the database to get the space
            back on disk.
          </p>
        </li>

        <!-- Models -->
        <li class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3">
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">Models</div>
            <div class="text-xs text-(--ui-text-muted) truncate" :title="status.models.path">
              {{ status.models.path }}
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">{{ formatBytes(status.models.bytes) }}</span>
        </li>

        <!-- Runtime packs -->
        <li
          v-for="runtime in status.runtimes"
          :key="runtime.ortVersion"
          class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3"
        >
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">
              Compute backends
              <span class="font-normal text-(--ui-text-muted)">
                · ONNX Runtime {{ runtime.ortVersion }}{{ runtime.current ? "" : " (unused)" }}
              </span>
            </div>
            <div class="text-xs text-(--ui-text-muted) truncate" :title="runtime.path">
              {{ runtime.path }}
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">{{ formatBytes(runtime.bytes) }}</span>
        </li>

        <!-- CoreML cache (macOS) -->
        <li
          v-if="status.coremlCache"
          class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3"
        >
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">CoreML cache</div>
            <div class="text-xs text-(--ui-text-muted)">
              Compiled models. Safe to clear; they are rebuilt when next needed.
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">
            {{ formatBytes(status.coremlCache.bytes) }}
          </span>
          <UButton
            size="xs"
            variant="soft"
            class="shrink-0"
            :disabled="status.coremlCache.bytes === 0"
            :loading="clear.isPending.value && clear.variables.value === 'coreml_cache'"
            @click="onClear('coreml_cache')"
          >
            Clear
          </UButton>
        </li>

        <!-- Logs -->
        <li class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3">
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">Logs</div>
            <div class="text-xs text-(--ui-text-muted)">Diagnostics only. Kept small automatically.</div>
          </div>
          <span class="tabular-nums text-(--ui-text)">{{ formatBytes(status.logs.bytes) }}</span>
        </li>

        <!-- Pre-update database backups -->
        <li
          v-if="status.databaseBackups.length > 0"
          class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3"
        >
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">Database backup</div>
            <div class="text-xs text-(--ui-text-muted)">
              A copy of the database from before the last update changed it. Delete it
              once you are happy the update went well.
            </div>
            <div
              v-for="backup in status.databaseBackups"
              :key="backup.name"
              class="text-xs text-(--ui-text-dimmed)"
            >
              {{ backup.name }} · {{ fmtDate(backup.modifiedAt) }}
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">
            {{ formatBytes(total(status.databaseBackups)) }}
          </span>
          <UButton
            size="xs"
            variant="soft"
            class="shrink-0"
            :loading="clear.isPending.value && clear.variables.value === 'database_backups'"
            @click="onClear('database_backups')"
          >
            Delete
          </UButton>
        </li>

        <!-- Set-aside WAL files -->
        <li
          v-if="status.setAsideWals.length > 0"
          class="rounded border border-(--ui-border) bg-(--ui-bg-elevated) px-3 py-2 flex items-center gap-3"
        >
          <div class="flex-1 min-w-0">
            <div class="text-(--ui-text) font-medium">Set-aside recovery files</div>
            <div class="text-xs text-(--ui-text-muted)">
              {{ status.setAsideWals.length }}
              {{ status.setAsideWals.length === 1 ? "file" : "files" }} kept after the database
              could not replay its log at startup. Only useful for diagnosing that problem.
            </div>
          </div>
          <span class="tabular-nums text-(--ui-text)">
            {{ formatBytes(total(status.setAsideWals)) }}
          </span>
          <UButton
            size="xs"
            variant="soft"
            class="shrink-0"
            :loading="clear.isPending.value && clear.variables.value === 'set_aside_wals'"
            @click="onClear('set_aside_wals')"
          >
            Delete
          </UButton>
        </li>
      </ul>
      <p v-if="clear.error.value" class="text-sm text-(--ui-color-error-500)">
        {{ clear.error.value.message }}
      </p>
    </template>

    <ResetAppData />

    <UModal v-model:open="pruneOpen" :title="pruneScope ? PRUNE_COPY[pruneScope].title : ''">
      <template #body>
        <div class="flex flex-col gap-3 text-sm">
          <p v-if="pruneScope" class="text-(--ui-text-muted)">{{ PRUNE_COPY[pruneScope].body }}</p>
          <p v-if="prune.error.value" class="text-(--ui-color-error-500)">
            Removing failed: {{ prune.error.value.message }}
          </p>
        </div>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2 w-full">
          <UButton variant="ghost" color="neutral" @click="pruneScope = null">Cancel</UButton>
          <UButton color="error" :loading="prune.isPending.value" @click="onPrune">
            Remove Results
          </UButton>
        </div>
      </template>
    </UModal>

    <UModal
      v-model:open="compactOpen"
      title="Compact the database?"
      :dismissible="!compact.isPending.value"
    >
      <template #body>
        <div class="flex flex-col gap-3 text-sm">
          <p class="text-(--ui-text-muted)">
            The app writes a fresh copy of the database that holds only what is in use,
            then relaunches to put it in place. Nothing is deleted. On a large database
            the copy can take a minute; imports and classifications can't start until the app has
            relaunched.
          </p>
          <p v-if="compact.error.value" class="text-(--ui-color-error-500)">
            {{ compact.error.value.message }}
          </p>
        </div>
      </template>
      <template #footer>
        <div class="flex justify-end gap-2 w-full">
          <UButton
            variant="ghost"
            color="neutral"
            :disabled="compact.isPending.value"
            @click="compactOpen = false"
          >
            Cancel
          </UButton>
          <UButton color="primary" :loading="compact.isPending.value" @click="compact.mutate()">
            Compact and Relaunch
          </UButton>
        </div>
      </template>
    </UModal>
  </section>
</template>
