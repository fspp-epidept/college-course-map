<script setup lang="ts">
import { useMutation, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { commands } from "../../bindings";
import type { OpenTab } from "../../stores/workspace";

const props = defineProps<{ tab: OpenTab }>();

// Tabs in this activity use ids shaped like `dataset:<uuid>`. Strip the prefix
// to get the dataset id this tab is bound to.
const datasetId = computed(() => props.tab.id.replace(/^dataset:/, ""));

const digitLevel = ref<2 | 4 | 6>(6);
const lastResult = ref<{
  rowsProcessed: number;
  uniqueInputsDone: number;
  cacheHits: number;
  durationMs: number;
  digitLevel: number;
} | null>(null);
const errorMessage = ref<string | null>(null);

const queryClient = useQueryClient();

const classify = useMutation({
  mutationFn: async (level: 2 | 4 | 6) => {
    const result = await commands.startRun({
      datasetId: datasetId.value,
      digitLevel: level,
      limit: 50,
    });
    if (result.status === "error") throw new Error(result.error);
    return { ...result.data, digitLevel: level };
  },
  onSuccess: (data) => {
    lastResult.value = data;
    errorMessage.value = null;
    // Refresh datasets so any new row_count / run state surfaces in the sidebar.
    queryClient.invalidateQueries({ queryKey: ["datasets"] });
    queryClient.invalidateQueries({ queryKey: ["metrics"] });
    queryClient.invalidateQueries({ queryKey: ["runs"] });
  },
  onError: (err: Error) => {
    errorMessage.value = err.message;
  },
});

function runFor(level: 2 | 4 | 6): void {
  classify.mutate(level);
}
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-6 overflow-auto">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-database" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ tab.label }}</h2>
    </header>

    <section class="flex flex-col gap-3">
      <h3 class="text-sm font-medium text-(--ui-text)">Classify</h3>
      <p class="text-sm text-(--ui-text-muted)">
        Run real Rust ONNX inference against the first 50 courses in this dataset.
        Results are cached by (model, content_hash) — re-running the same digit
        level will hit the cache instead of computing again.
      </p>
      <div class="flex items-center gap-2">
        <UButton
          v-for="level in [2, 4, 6] as const"
          :key="level"
          :color="digitLevel === level ? 'primary' : 'neutral'"
          :variant="digitLevel === level ? 'solid' : 'outline'"
          :loading="classify.isPending.value && digitLevel === level"
          :disabled="classify.isPending.value"
          @click="digitLevel = level; runFor(level)"
        >
          Classify ({{ level }}-digit)
        </UButton>
      </div>

      <div
        v-if="errorMessage"
        class="rounded-lg border border-(--ui-color-error-500)/40 bg-(--ui-color-error-500)/10 px-3 py-2 text-sm text-(--ui-color-error-500)"
      >
        {{ errorMessage }}
      </div>

      <div
        v-if="lastResult"
        class="rounded-lg border border-(--ui-border) bg-(--ui-bg-elevated) px-4 py-3 text-sm flex flex-col gap-1"
      >
        <div class="text-(--ui-text) font-medium">
          {{ lastResult.digitLevel }}-digit run complete
        </div>
        <div class="grid grid-cols-2 gap-x-6 gap-y-1 text-(--ui-text-muted)">
          <span>Rows processed</span>
          <span class="text-(--ui-text)">{{ lastResult.rowsProcessed }}</span>
          <span>New classifications</span>
          <span class="text-(--ui-text)">{{ lastResult.uniqueInputsDone }}</span>
          <span>Cache hits</span>
          <span class="text-(--ui-text)">{{ lastResult.cacheHits }}</span>
          <span>Duration</span>
          <span class="text-(--ui-text)">{{ lastResult.durationMs }} ms</span>
        </div>
      </div>
    </section>

    <p class="text-xs text-(--ui-text-dimmed)">
      Dataset id: <code>{{ datasetId }}</code>
    </p>
  </div>
</template>
