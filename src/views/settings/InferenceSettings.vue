<script setup lang="ts">
import { computed } from "vue";
import type { EpKind } from "../../bindings";
import {
  useDownloadRuntime,
  useEpPriority,
  useRuntimeEvents,
  useRuntimeStatus,
  useSetEpPriority,
} from "../../composables/useRuntime";

const EP_LABELS: Record<EpKind, string> = {
  tensorrt: "TensorRT",
  cuda: "CUDA",
  directml: "DirectML",
  coreml: "CoreML",
  cpu: "CPU",
};

const runtime = useRuntimeStatus();
const epPriority = useEpPriority();
const setEpPriority = useSetEpPriority();
const downloadRuntime = useDownloadRuntime();
const { progress } = useRuntimeEvents();

// Settings order when loaded, platform default as the pre-load placeholder.
const eps = computed<EpKind[]>(
  () => epPriority.data.value ?? runtime.data.value?.platformDefaultPriority ?? [],
);

function move(index: number, delta: -1 | 1): void {
  const next = [...eps.value];
  const target = index + delta;
  const item = next[index];
  const other = next[target];
  if (item === undefined || other === undefined) return;
  next[index] = other;
  next[target] = item;
  setEpPriority.mutate(next);
}

function fmtSize(bytes: number): string {
  return bytes >= 1_000_000_000
    ? `${(bytes / 1_000_000_000).toFixed(1)} GB`
    : `${Math.round(bytes / 1_000_000)} MB`;
}
</script>

<template>
  <section class="flex flex-col gap-6">
    <header>
      <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">Inference</h2>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Where classification runs. Providers are tried top to bottom when
        models load; the first one available on this machine wins, and
        anything below CPU never runs.
      </p>
    </header>

    <div
      class="rounded-lg border border-(--ui-border) p-4 bg-(--ui-bg-elevated) text-sm flex items-center gap-3"
    >
      <UIcon name="i-lucide-cpu" class="size-4 text-(--ui-text-muted)" />
      <template v-if="runtime.data.value">
        <span class="text-(--ui-text)">
          Active provider:
          <span class="font-medium">
            {{
              runtime.data.value.resolvedEp
                ? EP_LABELS[runtime.data.value.resolvedEp as EpKind] ??
                  runtime.data.value.resolvedEp
                : "models not loaded yet"
            }}
          </span>
        </span>
        <span class="text-(--ui-text-dimmed)">
          ONNX Runtime {{ runtime.data.value.ortVersion }} · pack
          "{{ runtime.data.value.activePackId }}"
        </span>
      </template>
      <span v-else class="text-(--ui-text-dimmed)">Loading…</span>
    </div>

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">Provider priority</h3>
      <p class="text-xs text-(--ui-text-muted)">
        Reordering reloads the models (a few seconds). A provider that fails to
        initialize is skipped — no configuration breaks classification.
      </p>
      <ul class="flex flex-col gap-1">
        <li
          v-for="(ep, index) in eps"
          :key="ep"
          class="flex items-center gap-2 rounded border border-(--ui-border) px-3 py-2 text-sm bg-(--ui-bg-elevated)"
        >
          <span class="w-5 text-xs text-(--ui-text-dimmed)">{{ index + 1 }}</span>
          <span class="flex-1 text-(--ui-text)">{{ EP_LABELS[ep] }}</span>
          <span
            v-if="ep === runtime.data.value?.resolvedEp"
            class="text-xs text-(--ui-color-primary-500)"
            >active</span
          >
          <UButton
            icon="i-lucide-chevron-up"
            variant="ghost"
            color="neutral"
            size="xs"
            :disabled="index === 0 || setEpPriority.isPending.value"
            aria-label="Move up"
            @click="move(index, -1)"
          />
          <UButton
            icon="i-lucide-chevron-down"
            variant="ghost"
            color="neutral"
            size="xs"
            :disabled="index === eps.length - 1 || setEpPriority.isPending.value"
            aria-label="Move down"
            @click="move(index, 1)"
          />
        </li>
      </ul>
    </div>

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">Runtime packs</h3>
      <p class="text-xs text-(--ui-text-muted)">
        GPU providers need their runtime pack downloaded once. A newly
        installed pack is used after the app is relaunched.
      </p>
      <ul class="flex flex-col gap-1">
        <li
          v-for="pack in runtime.data.value?.packs ?? []"
          :key="pack.id"
          class="flex items-center gap-3 rounded border border-(--ui-border) px-3 py-2 text-sm bg-(--ui-bg-elevated)"
        >
          <div class="flex-1">
            <span class="text-(--ui-text) font-medium">{{ pack.id }}</span>
            <span class="ml-2 text-xs text-(--ui-text-muted)">
              {{ pack.eps.map((ep) => EP_LABELS[ep as EpKind] ?? ep).join(" + ") }}
              · {{ fmtSize(pack.sizeBytes) }}
            </span>
          </div>
          <template v-if="progress[pack.id] && !pack.installed">
            <UProgress
              :model-value="(progress[pack.id]!.received / progress[pack.id]!.total) * 100"
              class="w-32"
            />
            <span class="text-xs text-(--ui-text-dimmed) w-20 text-right">
              {{ (progress[pack.id]!.bytesPerSec / 1_000_000).toFixed(1) }} MB/s
            </span>
          </template>
          <span v-else-if="pack.active" class="text-xs text-(--ui-color-primary-500)">
            active
          </span>
          <span v-else-if="pack.installed" class="text-xs text-(--ui-text-dimmed)">
            installed — relaunch to use
          </span>
          <UButton
            v-else
            size="xs"
            variant="soft"
            :loading="downloadRuntime.isPending.value"
            @click="downloadRuntime.mutate(pack.id)"
          >
            Download
          </UButton>
        </li>
      </ul>
      <p v-if="downloadRuntime.error.value" class="text-xs text-(--ui-color-error-500)">
        {{ downloadRuntime.error.value.message }}
      </p>
    </div>
  </section>
</template>
