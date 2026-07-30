<script setup lang="ts">
import { computed } from "vue";
import {
  useCancelDownload,
  useDownloadModels,
  useDownloading,
  useLoadModels,
  useModelsEvents,
  useModelsStatus,
} from "../../composables/useModels";

const { data: models, isPending, isError, error } = useModelsStatus();
const { progress } = useModelsEvents();
const download = useDownloadModels();
const cancel = useCancelDownload();
// Backend truth (EPI-74): reflects downloads started before this mount or
// from anywhere else; the mutation's isPending only covers this component's
// own invocation window before the status refetch lands.
const backendDownloading = useDownloading();
const downloading = computed(() => backendDownloading.value || download.isPending.value);
const load = useLoadModels();

const allPresent = computed(
  () => models.value?.every((m) => m.filesPresent === m.filesTotal) ?? false,
);
const anyMissing = computed(
  () => models.value?.some((m) => m.filesPresent < m.filesTotal) ?? false,
);
const loaded = computed(() => models.value?.[0]?.loaded ?? false);
const loading = computed(() => models.value?.[0]?.loading ?? false);

function fmtGb(bytes: number): string {
  return `${(bytes / 1e9).toFixed(2)} GB`;
}

function fmtMb(bytes: number): string {
  return `${Math.round(bytes / 1e6)} MB`;
}

/** Live event progress when we have it, else the backend snapshot from
 * `models_status` — which is what a freshly-mounted panel sees mid-download
 * before the next event tick (EPI-74). */
function progressFor(m: {
  digitLevel: number;
  download: { file: string; received: number; total: number } | null;
}): { file: string; received: number; total: number; bytesPerSec: number | null } | null {
  const live = progress.value[m.digitLevel];
  if (live) return live;
  return m.download ? { ...m.download, bytesPerSec: null } : null;
}

function pct(m: Parameters<typeof progressFor>[0]): number | null {
  const p = progressFor(m);
  if (!p || p.total === 0) return null;
  return Math.round((p.received / p.total) * 100);
}

function downloadDetail(m: Parameters<typeof progressFor>[0]): string | null {
  const p = progressFor(m);
  if (!p) return null;
  const speed = p.bytesPerSec === null ? "" : ` · ${(p.bytesPerSec / 1e6).toFixed(1)} MB/s`;
  return `${p.file} — ${fmtMb(p.received)} / ${fmtMb(p.total)}${speed}`;
}

function statusLabel(m: {
  digitLevel: number;
  filesPresent: number;
  filesTotal: number;
  download: { file: string; received: number; total: number } | null;
}): string {
  if (loaded.value) return "loaded";
  const p = pct(m);
  if (downloading.value && p !== null && m.filesPresent < m.filesTotal) {
    return `downloading ${p}%`;
  }
  if (m.filesPresent === m.filesTotal) return "on disk";
  if (m.filesPresent === 0) return "missing";
  return `${m.filesPresent}/${m.filesTotal} files`;
}
</script>

<template>
  <div class="h-full p-8 flex flex-col gap-4 overflow-auto">
    <header>
      <h1 class="text-2xl font-semibold text-(--ui-text-highlighted)">Models</h1>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        CCM classifiers pinned by the build manifest — downloaded once, verified
        by sha256, loaded into ONNX Runtime for classification.
      </p>
    </header>

    <p v-if="isError" class="text-sm text-(--ui-color-error-500)">
      Failed to read model status: {{ error?.message }}
    </p>
    <p v-else-if="isPending" class="text-sm text-(--ui-text-dimmed)">Reading model status…</p>

    <template v-else-if="models">
      <div class="flex items-center gap-2">
        <template v-if="anyMissing">
          <UButton
            color="primary"
            icon="i-lucide-download"
            :loading="downloading"
            :disabled="downloading"
            @click="download.mutate()"
          >
            Download models
          </UButton>
          <UButton
            v-if="downloading"
            color="neutral"
            variant="subtle"
            icon="i-lucide-x"
            :loading="cancel.isPending.value"
            @click="cancel.mutate()"
          >
            Cancel
          </UButton>
        </template>
        <UButton
          v-else-if="!loaded"
          color="primary"
          icon="i-lucide-play"
          :loading="load.isPending.value || loading"
          @click="load.mutate()"
        >
          Load models
        </UButton>
        <span v-else class="text-sm text-(--ui-text-muted)">
          All models loaded — classification is ready.
        </span>
      </div>

      <p v-if="download.isError.value" class="text-sm text-(--ui-color-error-500)">
        Download failed: {{ download.error.value?.message }}
      </p>
      <p v-if="load.isError.value" class="text-sm text-(--ui-color-error-500)">
        Load failed: {{ load.error.value?.message }}
      </p>

      <div class="flex flex-col gap-3">
        <div
          v-for="m in models"
          :key="m.digitLevel"
          class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 flex flex-col gap-2"
        >
          <div class="flex items-baseline justify-between gap-3">
            <span class="text-sm font-medium text-(--ui-text)">{{ m.displayName }}</span>
            <span class="text-xs text-(--ui-text-muted) tabular-nums">
              {{ statusLabel(m) }}
            </span>
          </div>
          <template v-if="downloading && pct(m) !== null && m.filesPresent < m.filesTotal">
            <UProgress :model-value="pct(m) ?? 0" :max="100" size="sm" />
            <span class="text-xs text-(--ui-text-dimmed) tabular-nums">
              {{ downloadDetail(m) }}
            </span>
          </template>
          <div class="text-xs text-(--ui-text-dimmed) flex flex-wrap gap-x-4">
            <span>{{ m.hfRepo }}</span>
            <span class="font-mono">{{ m.revision.slice(0, 12) }}</span>
            <span class="tabular-nums">{{ fmtGb(m.totalBytes) }}</span>
          </div>
        </div>
      </div>

      <p v-if="allPresent && !loaded && !loading" class="text-xs text-(--ui-text-dimmed)">
        Files are on disk; loading takes ~15 seconds and happens automatically
        on the next launch.
      </p>
    </template>
  </div>
</template>
