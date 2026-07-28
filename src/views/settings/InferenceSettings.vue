<script setup lang="ts">
import { open } from "@tauri-apps/plugin-dialog";
import { computed, ref, watchEffect } from "vue";
import type { EpKind } from "../../bindings";
import {
  useApplyInferenceSettings,
  useDownloadRuntime,
  useInferenceSettings,
  useRuntimeEvents,
  useRuntimeStatus,
  useSaveRelaunchSetting,
} from "../../composables/useRuntime";

const EP_LABELS: Record<EpKind, string> = {
  tensorrt: "TensorRT",
  cuda: "CUDA",
  directml: "DirectML",
  coreml: "CoreML",
  cpu: "CPU",
};

const runtime = useRuntimeStatus();
const settings = useInferenceSettings();
const apply = useApplyInferenceSettings();
const saveRelaunch = useSaveRelaunchSetting();
const downloadRuntime = useDownloadRuntime();
const { progress } = useRuntimeEvents();

// Settings order when loaded, platform default as the pre-load placeholder.
const eps = computed<EpKind[]>(
  () =>
    settings.data.value?.executionProviders ?? runtime.data.value?.platformDefaultPriority ?? [],
);

function move(index: number, delta: -1 | 1): void {
  const next = [...eps.value];
  const target = index + delta;
  const item = next[index];
  const other = next[target];
  if (item === undefined || other === undefined) return;
  next[index] = other;
  next[target] = item;
  apply.mutate({ executionProviders: next });
}

// Local input state so typing doesn't fire a model reload per keystroke;
// committed on change (blur/enter).
const threadsInput = ref<number>(0);
watchEffect(() => {
  threadsInput.value = settings.data.value?.maxCpuThreads ?? 0;
});
function commitThreads(): void {
  const n = Number.isFinite(threadsInput.value) ? Math.trunc(threadsInput.value) : 0;
  if (n === (settings.data.value?.maxCpuThreads ?? 0)) return;
  apply.mutate({ maxCpuThreads: Math.max(0, n) });
}

const cudaDir = computed(() => settings.data.value?.cudaLibraryDir ?? null);
async function pickCudaDir(): Promise<void> {
  const dir = await open({ directory: true, multiple: false });
  if (typeof dir === "string") saveRelaunch.mutate({ cudaLibraryDir: dir });
}
function clearCudaDir(): void {
  saveRelaunch.mutate({ cudaLibraryDir: null });
}

// Backend startup notices (EPI-87: damaged pack, stale CUDA dir, missing
// libs pack) plus the one condition only the frontend can see: a GPU pack
// loaded but its provider failed to register at model load (driver too old,
// wrong CUDA generation) and inference silently resolved to CPU.
const notices = computed<string[]>(() => {
  const status = runtime.data.value;
  if (!status) return [];
  const all = [...status.notices];
  if (status.activePackId !== "cpu" && status.resolvedEp === "cpu") {
    all.push(
      `The "${status.activePackId}" pack is loaded but its GPU provider did not ` +
        "register, so inference is running on CPU. Check that the NVIDIA driver " +
        "is recent enough for this CUDA version.",
    );
  }
  return all;
});

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
        Where and how classification runs.
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

    <UAlert
      v-for="notice in notices"
      :key="notice"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :description="notice"
    />

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">Provider priority</h3>
      <p class="text-xs text-(--ui-text-muted)">
        Providers are tried top to bottom when models load; the first one
        available on this machine wins, and anything below CPU never runs.
        Reordering reloads the models (a few seconds).
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
            :disabled="index === 0 || apply.isPending.value"
            aria-label="Move up"
            @click="move(index, -1)"
          />
          <UButton
            icon="i-lucide-chevron-down"
            variant="ghost"
            color="neutral"
            size="xs"
            :disabled="index === eps.length - 1 || apply.isPending.value"
            aria-label="Move down"
            @click="move(index, 1)"
          />
        </li>
      </ul>
    </div>

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">CPU</h3>
      <div class="flex items-center gap-3">
        <label class="text-sm text-(--ui-text)" for="max-cpu-threads">
          Max CPU threads
        </label>
        <UInput
          id="max-cpu-threads"
          v-model.number="threadsInput"
          type="number"
          min="0"
          class="w-24"
          :disabled="apply.isPending.value"
          @blur="commitThreads"
          @keydown.enter="commitThreads"
        />
        <span class="text-xs text-(--ui-text-muted)">
          0 = all cores. Values above the machine's core count also use all
          cores. Applies on the next model reload (automatic on change).
        </span>
      </div>
    </div>

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">Runtime packs</h3>
      <p class="text-xs text-(--ui-text-muted)">
        GPU providers need their runtime pack downloaded once — and CUDA needs
        its support libraries, either the downloadable pack below or a
        directory you point at. Newly installed packs are used after the app
        is relaunched.
      </p>
      <ul class="flex flex-col gap-1">
        <li
          v-for="pack in runtime.data.value?.packs ?? []"
          :key="pack.id"
          class="flex items-center gap-3 rounded border border-(--ui-border) px-3 py-2 text-sm bg-(--ui-bg-elevated)"
        >
          <div class="flex-1">
            <span class="text-(--ui-text) font-medium">{{ pack.displayName }}</span>
            <span class="ml-2 text-xs text-(--ui-text-muted)">
              <template v-if="pack.eps.length">
                {{ pack.eps.map((ep) => EP_LABELS[ep as EpKind] ?? ep).join(" + ") }} ·
              </template>
              {{ fmtSize(pack.sizeBytes) }}
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

    <div class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-(--ui-text)">CUDA library directory</h3>
      <p class="text-xs text-(--ui-text-muted)">
        Already have CUDA in a conda or Python environment? Point at its
        <code>nvidia</code> libraries directory instead of downloading the
        support pack. Takes effect after relaunch.
      </p>
      <div class="flex items-center gap-2">
        <code
          class="flex-1 truncate rounded border border-(--ui-border) px-3 py-1.5 text-xs bg-(--ui-bg-elevated) text-(--ui-text-muted)"
        >
          {{ cudaDir ?? "not set" }}
        </code>
        <UButton size="xs" variant="soft" @click="pickCudaDir">Browse…</UButton>
        <UButton
          v-if="cudaDir"
          size="xs"
          variant="ghost"
          color="neutral"
          @click="clearCudaDir"
        >
          Clear
        </UButton>
      </div>
    </div>
  </section>
</template>
