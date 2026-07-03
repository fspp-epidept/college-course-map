<script setup lang="ts">
import { computed } from "vue";
import {
  useDownloadModels,
  useLoadModels,
  useModelsEvents,
  useModelsStatus,
} from "../../composables/useModels";

const { data: models, isPending, isError, error } = useModelsStatus();
const { progress } = useModelsEvents();
const download = useDownloadModels();
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

function pct(digitLevel: number): number | null {
  const p = progress.value[digitLevel];
  if (!p || p.total === 0) return null;
  return Math.round((p.received / p.total) * 100);
}

function downloadDetail(digitLevel: number): string | null {
  const p = progress.value[digitLevel];
  if (!p) return null;
  return `${p.file} — ${fmtMb(p.received)} / ${fmtMb(p.total)} · ${(p.bytesPerSec / 1e6).toFixed(1)} MB/s`;
}

function statusLabel(m: { digitLevel: number; filesPresent: number; filesTotal: number }): string {
  if (loaded.value) return "loaded";
  const p = pct(m.digitLevel);
  if (download.isPending.value && p !== null && m.filesPresent < m.filesTotal) {
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
        <UButton
          v-if="anyMissing"
          color="primary"
          icon="i-lucide-download"
          :loading="download.isPending.value"
          @click="download.mutate()"
        >
          Download models
        </UButton>
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
          <template
            v-if="download.isPending.value && pct(m.digitLevel) !== null && m.filesPresent < m.filesTotal"
          >
            <UProgress :model-value="pct(m.digitLevel) ?? 0" :max="100" size="sm" />
            <span class="text-xs text-(--ui-text-dimmed) tabular-nums">
              {{ downloadDetail(m.digitLevel) }}
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
