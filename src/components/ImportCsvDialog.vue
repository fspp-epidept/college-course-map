<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { open } from "@tauri-apps/plugin-dialog";
import { computed, ref, watch } from "vue";
import { type CsvPreview, commands } from "../bindings";

const isOpen = defineModel<boolean>("open", { default: false });

const path = ref<string | null>(null);
const preview = ref<CsvPreview | null>(null);
const previewError = ref<string | null>(null);
const previewing = ref(false);
const displayName = ref<string>("");
const importError = ref<string | null>(null);

const queryClient = useQueryClient();

const importMutation = useMutation({
  mutationFn: async (request: { path: string; displayName: string | null }) => {
    const result = await commands.importCsv({
      path: request.path,
      displayName: request.displayName,
      // null = import every row. The Rust command bounds file size + field
      // length + column count itself; we no longer cap row count here.
      limit: null,
    });
    if (result.status === "error") throw new Error(result.error);
    return result.data;
  },
  onSuccess: () => {
    // `importCsv` now returns as soon as the dataset row is inserted; the
    // background worker streams rows in. The sidebar's useDatasets refetches
    // every 500 ms while any dataset is `importing`, so closing the modal
    // is the right move: the user can watch the row count tick up live.
    queryClient.invalidateQueries({ queryKey: ["datasets"] });
    queryClient.invalidateQueries({ queryKey: ["metrics"] });
    importError.value = null;
    isOpen.value = false;
  },
  onError: (err: Error) => {
    importError.value = err.message;
  },
});

const fileLabel = computed(() => {
  if (!path.value) return null;
  const segments = path.value.split(/[\\/]/);
  return segments[segments.length - 1] ?? path.value;
});

const sizeLabel = computed(() => {
  if (!preview.value) return null;
  const bytes = preview.value.sizeBytes;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GiB`;
});

async function pickFile(): Promise<void> {
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  if (typeof picked !== "string") return; // user cancelled
  path.value = picked;
  preview.value = null;
  previewError.value = null;
  importError.value = null;
  previewing.value = true;
  const base = picked.split(/[\\/]/).pop() ?? picked;
  displayName.value = base.replace(/\.csv$/i, "");
  try {
    const result = await commands.previewCsv(picked);
    if (result.status === "error") {
      previewError.value = result.error;
    } else {
      preview.value = result.data;
    }
  } finally {
    previewing.value = false;
  }
}

function reset(): void {
  path.value = null;
  preview.value = null;
  previewError.value = null;
  importError.value = null;
  displayName.value = "";
}

function importNow(): void {
  if (!path.value) return;
  importMutation.mutate({
    path: path.value,
    displayName: displayName.value.trim() || null,
  });
}

// Reset internal state every time the modal opens so a stale preview from a
// previous session doesn't flash on screen.
watch(isOpen, (next) => {
  if (next) reset();
});
</script>

<template>
  <UModal v-model:open="isOpen" title="Import CSV" :ui="{ content: 'max-w-3xl' }">
    <template #body>
      <div class="flex flex-col gap-4">
        <div class="flex items-center gap-3">
          <UButton
            color="primary"
            variant="solid"
            icon="i-lucide-file-up"
            :loading="previewing"
            @click="pickFile"
          >
            Choose CSV…
          </UButton>
          <div v-if="fileLabel" class="flex flex-col text-sm min-w-0">
            <span class="truncate text-(--ui-text)">{{ fileLabel }}</span>
            <span v-if="sizeLabel" class="text-xs text-(--ui-text-dimmed)">{{ sizeLabel }}</span>
          </div>
        </div>

        <p v-if="previewError" class="text-sm text-(--ui-color-error-500)">
          Preview failed: {{ previewError }}
        </p>

        <div v-if="preview" class="flex flex-col gap-3">
          <label class="flex flex-col gap-1 text-sm">
            <span class="text-(--ui-text-muted)">Display name</span>
            <UInput v-model="displayName" placeholder="Dataset name" />
          </label>

          <div class="rounded-lg border border-(--ui-border) overflow-hidden">
            <div class="overflow-x-auto max-h-80">
              <table class="min-w-full text-xs">
                <thead class="bg-(--ui-bg-muted) sticky top-0">
                  <tr>
                    <th
                      v-for="(h, i) in preview.headers"
                      :key="i"
                      class="px-2 py-1.5 text-left font-medium text-(--ui-text) whitespace-nowrap"
                    >
                      {{ h }}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="(row, ri) in preview.sampleRows"
                    :key="ri"
                    class="border-t border-(--ui-border-muted)"
                  >
                    <td
                      v-for="(cell, ci) in row"
                      :key="ci"
                      class="px-2 py-1 text-(--ui-text-muted) whitespace-nowrap"
                    >
                      {{ cell }}
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>

          <p class="text-xs text-(--ui-text-dimmed)">
            {{ preview.totalColumns }} columns · showing first {{ preview.sampleRows.length }} rows.
            Import auto-detects subject/catalog/title columns by header name and
            ingests every row in the file. The import runs in the background;
            you'll see the row count tick up live in the sidebar.
          </p>
        </div>

        <p v-if="importError" class="text-sm text-(--ui-color-error-500)">
          Import failed: {{ importError }}
        </p>
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="isOpen = false">Close</UButton>
        <UButton
          color="primary"
          :disabled="!preview || importMutation.isPending.value"
          :loading="importMutation.isPending.value"
          @click="importNow"
        >
          Import
        </UButton>
      </div>
    </template>
  </UModal>
</template>
