<script setup lang="ts">
import { computed, ref } from "vue";
import { commands } from "../../bindings";
import { useWorkspace } from "../../stores/workspace";
import CcmDetail from "./CcmDetail.vue";

// Master/detail like Datasets/Runs: the sidebar owns selection, `:key` gives
// each code a fresh detail instance.
const workspace = useWorkspace();
const code = computed(() => workspace.selectedCcmCode);

const openError = ref<string | null>(null);

// Rust opens the bundled PDF so the WebView needs no opener permission. Fails
// on e.g. Linux with no PDF viewer registered; say so rather than no-op.
async function openReport(): Promise<void> {
  const result = await commands.openCcmReference();
  openError.value = result.status === "error" ? result.error : null;
}
</script>

<template>
  <div class="h-full flex flex-col">
    <header class="flex items-center justify-between gap-3 px-4 py-2 border-b border-(--ui-border)">
      <span class="text-sm text-(--ui-text-muted)">2010 College Course Map (NCES 2012-162rev)</span>
      <UButton size="xs" variant="outline" icon="i-lucide-file-text" @click="openReport">
        Open report (PDF)
      </UButton>
    </header>
    <p v-if="openError" class="px-4 py-2 text-sm text-(--ui-color-error-500)">
      Couldn’t open the report: {{ openError }}
    </p>
    <div class="flex-1 min-h-0 overflow-auto">
      <CcmDetail v-if="code" :key="code" :code="code" />
      <div
        v-else
        class="h-full flex flex-col items-center justify-center text-center gap-2 text-(--ui-text-dimmed)"
      >
        <UIcon name="i-lucide-book-open" class="size-10" />
        <p class="text-sm">Select a CCM family or code from the sidebar.</p>
        <p class="text-xs">Search by code, title keywords, or description keywords.</p>
      </div>
    </div>
  </div>
</template>
