<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { open } from "@tauri-apps/plugin-dialog";
import { computed, ref, watch } from "vue";
import { commands, type Inspection, type TextEncoding, type Validation } from "../bindings";

const isOpen = defineModel<boolean>("open", { default: false });

// Pre-flight runs in two passes (see src-tauri/src/preflight.rs): inspect
// settles the encoding on file pick, validate is the decoded dry run that
// yields the preview, mapping, and per-column stats. Import only unlocks
// after a successful validate.
const path = ref<string | null>(null);
const inspection = ref<Inspection | null>(null);
const inspecting = ref(false);
const encoding = ref<TextEncoding | null>(null);
const validation = ref<Validation | null>(null);
const validating = ref(false);
const preflightError = ref<string | null>(null);
const displayName = ref<string>("");
const importError = ref<string | null>(null);

const queryClient = useQueryClient();

const importMutation = useMutation({
  mutationFn: async (request: {
    path: string;
    displayName: string | null;
    encoding: TextEncoding;
    validation: Validation;
  }) => {
    const result = await commands.importCsv({
      path: request.path,
      displayName: request.displayName,
      // null = import every row. The Rust command bounds file size + field
      // length + column count itself; we no longer cap row count here.
      limit: null,
      encoding: request.encoding,
      mapping: request.validation.mapping,
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

const ENCODING_NAMES: Record<TextEncoding, string> = {
  utf8: "UTF-8",
  windows1252: "Windows-1252",
  macRoman: "Mac Roman",
};

const fileLabel = computed(() => {
  if (!path.value) return null;
  const segments = path.value.split(/[\\/]/);
  return segments[segments.length - 1] ?? path.value;
});

const sizeLabel = computed(() => {
  if (!inspection.value) return null;
  const bytes = inspection.value.sizeBytes;
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GiB`;
});

const report = computed(() => inspection.value?.encoding ?? null);
const invalidFields = computed(() =>
  report.value && report.value.kind !== "utf8" ? report.value.invalid : null,
);
const blocked = computed(
  () =>
    !!inspection.value &&
    (inspection.value.encoding.kind === "mixed" || inspection.value.raggedRows.count > 0),
);
const needsEncodingChoice = computed(
  () => !blocked.value && report.value?.kind === "legacy" && encoding.value === null,
);
const mappedColumns = computed(() => {
  const v = validation.value;
  if (!v) return [];
  return [
    { label: "Subject", stats: v.columns.subject },
    { label: "Catalog number", stats: v.columns.catalog },
    { label: "Title", stats: v.columns.title },
  ];
});
const canImport = computed(
  () => !!validation.value && validation.value.importable > 0 && !importMutation.isPending.value,
);

function n(value: number): string {
  return value.toLocaleString();
}

async function pickFile(): Promise<void> {
  const picked = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  if (typeof picked !== "string") return; // user cancelled
  reset();
  path.value = picked;
  const base = picked.split(/[\\/]/).pop() ?? picked;
  displayName.value = base.replace(/\.csv$/i, "");
  inspecting.value = true;
  try {
    const result = await commands.inspectCsv(picked);
    if (result.status === "error") {
      preflightError.value = result.error;
      return;
    }
    inspection.value = result.data;
  } finally {
    inspecting.value = false;
  }
  if (!blocked.value && inspection.value?.encoding.kind === "utf8") {
    await chooseEncoding("utf8");
  }
}

async function chooseEncoding(choice: TextEncoding): Promise<void> {
  if (!path.value) return;
  encoding.value = choice;
  validation.value = null;
  preflightError.value = null;
  validating.value = true;
  try {
    const result = await commands.validateImport(path.value, choice);
    if (result.status === "error") {
      preflightError.value = result.error;
    } else {
      validation.value = result.data;
    }
  } finally {
    validating.value = false;
  }
}

function reset(): void {
  path.value = null;
  inspection.value = null;
  encoding.value = null;
  validation.value = null;
  preflightError.value = null;
  importError.value = null;
  displayName.value = "";
}

function importNow(): void {
  if (!path.value || !encoding.value || !validation.value) return;
  importMutation.mutate({
    path: path.value,
    displayName: displayName.value.trim() || null,
    encoding: encoding.value,
    validation: validation.value,
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
            :loading="inspecting || validating"
            @click="pickFile"
          >
            Choose CSV…
          </UButton>
          <div v-if="fileLabel" class="flex flex-col text-sm min-w-0">
            <span class="truncate text-(--ui-text)">{{ fileLabel }}</span>
            <span v-if="sizeLabel" class="text-xs text-(--ui-text-dimmed)">
              {{ sizeLabel }}
              <template v-if="inspection"> · {{ n(inspection.rows) }} rows</template>
              <template v-if="encoding"> · {{ ENCODING_NAMES[encoding] }}</template>
            </span>
          </div>
        </div>

        <p v-if="inspecting" class="text-sm text-(--ui-text-muted)">Checking the file…</p>

        <p v-if="preflightError" class="text-sm text-(--ui-color-error-500)">
          Can't import this file: {{ preflightError }}
        </p>

        <div
          v-if="inspection && inspection.raggedRows.count > 0"
          class="flex flex-col gap-1 text-sm"
        >
          <p class="text-(--ui-color-error-500)">
            Can't import this file: {{ n(inspection.raggedRows.count) }} of
            {{ n(inspection.rows) }} rows don't have {{ inspection.headerFields }} columns like
            the header row. Fix these rows (often an unquoted comma) and choose the file again.
          </p>
          <ul class="text-xs text-(--ui-text-muted)">
            <li v-for="r in inspection.raggedRows.first" :key="r.row">
              row {{ n(r.row) }}: {{ r.fields }} columns
            </li>
            <li v-if="inspection.raggedRows.count > inspection.raggedRows.first.length">
              … and {{ n(inspection.raggedRows.count - inspection.raggedRows.first.length) }} more
            </li>
          </ul>
        </div>

        <div v-if="inspection && invalidFields" class="flex flex-col gap-2 text-sm">
          <p v-if="report?.kind === 'mixed'" class="text-(--ui-color-error-500)">
            Can't import this file: it mixes text encodings. {{ n(invalidFields.count) }} values
            aren't UTF-8, but {{ n(report.utf8NonAsciiFields) }} others are, so no single encoding
            reads every row correctly. Fix the rows below in the source and re-save the whole
            file as "CSV UTF-8".
          </p>
          <p v-else class="text-(--ui-text)">
            This file isn't UTF-8. {{ n(invalidFields.count) }}
            {{ invalidFields.count === 1 ? "value needs" : "values need" }} a different encoding.
            Choose the one where the text reads correctly, or re-save the file as "CSV UTF-8" and
            choose it again.
          </p>

          <div class="rounded-lg border border-(--ui-border) overflow-hidden">
            <div class="overflow-x-auto max-h-60">
              <table class="min-w-full text-xs">
                <thead class="bg-(--ui-bg-muted) sticky top-0">
                  <tr>
                    <th class="px-2 py-1.5 text-left font-medium">Row</th>
                    <th class="px-2 py-1.5 text-left font-medium">Column</th>
                    <th class="px-2 py-1.5 text-left font-medium">As Windows-1252</th>
                    <th class="px-2 py-1.5 text-left font-medium">As Mac Roman</th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="f in invalidFields.first"
                    :key="`${f.row}-${f.column}`"
                    class="border-t border-(--ui-border-muted)"
                  >
                    <td class="px-2 py-1 whitespace-nowrap">{{ n(f.row) }}</td>
                    <td class="px-2 py-1 whitespace-nowrap">{{ f.column }}</td>
                    <td class="px-2 py-1 whitespace-nowrap">{{ f.windows1252 }}</td>
                    <td class="px-2 py-1 whitespace-nowrap">{{ f.macRoman }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </div>
          <p
            v-if="invalidFields.count > invalidFields.first.length"
            class="text-xs text-(--ui-text-dimmed)"
          >
            … and {{ n(invalidFields.count - invalidFields.first.length) }} more
          </p>

          <div v-if="report?.kind === 'legacy' && !blocked" class="flex gap-2">
            <UButton
              :variant="encoding === 'windows1252' ? 'solid' : 'outline'"
              :loading="validating && encoding === 'windows1252'"
              @click="chooseEncoding('windows1252')"
            >
              Use Windows-1252 (Excel on Windows)
            </UButton>
            <UButton
              :variant="encoding === 'macRoman' ? 'solid' : 'outline'"
              :loading="validating && encoding === 'macRoman'"
              @click="chooseEncoding('macRoman')"
            >
              Use Mac Roman (Excel on Mac)
            </UButton>
          </div>
        </div>

        <p v-if="validating && !needsEncodingChoice" class="text-sm text-(--ui-text-muted)">
          Checking every row…
        </p>

        <div v-if="validation" class="flex flex-col gap-3">
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
                      v-for="(h, i) in validation.headers"
                      :key="i"
                      class="px-2 py-1.5 text-left font-medium text-(--ui-text) whitespace-nowrap"
                    >
                      {{ h }}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr
                    v-for="(row, ri) in validation.sampleRows"
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

          <div class="flex flex-col gap-1 text-sm">
            <p class="text-(--ui-text)">
              {{ n(validation.importable) }} of {{ n(validation.rows) }} rows will be imported.
              <template v-if="validation.skipped.count > 0">
                {{ n(validation.skipped.count) }} will be skipped because a required column is
                empty.
              </template>
            </p>
            <ul v-if="validation.skipped.count > 0" class="text-xs text-(--ui-text-muted)">
              <li v-for="s in validation.skipped.first" :key="s.row">
                row {{ n(s.row) }}: empty {{ s.missing.join(", ") }}
              </li>
              <li v-if="validation.skipped.count > validation.skipped.first.length">
                … and {{ n(validation.skipped.count - validation.skipped.first.length) }} more
              </li>
            </ul>
            <p v-if="validation.truncatedFields > 0" class="text-xs text-(--ui-text-muted)">
              {{ n(validation.truncatedFields) }} values exceed 8 KiB and will be truncated.
            </p>
          </div>

          <div class="grid grid-cols-1 sm:grid-cols-3 gap-3 text-xs">
            <div v-for="c in mappedColumns" :key="c.label" class="flex flex-col gap-1">
              <span class="font-medium text-(--ui-text)">{{ c.label }}: {{ c.stats.header }}</span>
              <span class="text-(--ui-text-dimmed)">
                {{ c.stats.distinctCapped ? "over " : "" }}{{ n(c.stats.distinct) }} distinct ·
                {{ n(c.stats.empty) }} empty
              </span>
              <ul class="text-(--ui-text-muted)">
                <li v-for="v in c.stats.top" :key="v.value" class="flex justify-between gap-2">
                  <span class="truncate">{{ v.value }}</span>
                  <span>{{ n(v.count) }}</span>
                </li>
              </ul>
            </div>
          </div>

          <p class="text-xs text-(--ui-text-dimmed)">
            Subject, catalog, and title columns are detected by header name. The import runs in
            the background; you'll see the row count tick up live in the sidebar.
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
          :disabled="!canImport"
          :loading="importMutation.isPending.value"
          @click="importNow"
        >
          Import
        </UButton>
      </div>
    </template>
  </UModal>
</template>
