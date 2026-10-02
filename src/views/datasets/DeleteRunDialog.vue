<script setup lang="ts">
import { computed } from "vue";
import { useDeleteRun } from "../../composables/useRuns";

// Confirm for deleting a run's record (#198). Open while `runId` is set;
// closing or a finished delete clears it.
const runId = defineModel<string | null>("runId", { required: true });
const open = computed({
  get: () => runId.value !== null,
  set: (value) => {
    if (!value) runId.value = null;
  },
});

const deleteRun = useDeleteRun();

function onDelete(): void {
  if (runId.value === null) return;
  deleteRun.mutate(runId.value, {
    onSuccess: () => {
      runId.value = null;
    },
  });
}
</script>

<template>
  <UModal v-model:open="open" title="Delete this run?">
    <template #body>
      <div class="flex flex-col gap-3 text-sm">
        <p class="text-(--ui-text-muted)">
          This removes the run's record. The classifications it computed are kept and
          reused by the next run on the same courses.
        </p>
        <p v-if="deleteRun.error.value" class="text-(--ui-color-error-500)">
          Delete failed: {{ deleteRun.error.value.message }}
        </p>
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="open = false">Cancel</UButton>
        <UButton color="error" :loading="deleteRun.isPending.value" @click="onDelete">
          Delete Run
        </UButton>
      </div>
    </template>
  </UModal>
</template>
