<script setup lang="ts">
import { useDeleteDataset } from "../../composables/useDatasets";
import { useWorkspace } from "../../stores/workspace";

// Confirm for deleting a dataset (#199). `incomplete` is a dataset whose
// earlier delete was cut off: its courses and runs are already gone, so the
// dialog only finishes the job.
const props = defineProps<{
  datasetId: string;
  title: string;
  courseCount: number;
  runCount: number;
  incomplete: boolean;
}>();
const open = defineModel<boolean>("open", { required: true });

const workspace = useWorkspace();
const deleteDataset = useDeleteDataset();

function onDelete(): void {
  deleteDataset.mutate(props.datasetId, {
    onSuccess: () => {
      open.value = false;
      if (workspace.selectedDatasetId === props.datasetId) workspace.selectDataset(null);
    },
  });
}
</script>

<template>
  <UModal v-model:open="open" :title="incomplete ? 'Finish deleting this dataset?' : 'Delete this dataset?'">
    <template #body>
      <div class="flex flex-col gap-3 text-sm">
        <p v-if="incomplete" class="text-(--ui-text-muted)">
          Deleting <span class="text-(--ui-text)">{{ title }}</span> was interrupted. Its
          courses and runs are already gone; this removes what is left of it.
        </p>
        <template v-else>
          <p class="text-(--ui-text-muted)">
            This deletes <span class="text-(--ui-text)">{{ title }}</span> from the app:
            <span class="text-(--ui-text) tabular-nums">{{ courseCount.toLocaleString() }}</span>
            {{ courseCount === 1 ? "course" : "courses" }} and
            <span class="text-(--ui-text) tabular-nums">{{ runCount.toLocaleString() }}</span>
            {{ runCount === 1 ? "run" : "runs" }}. This cannot be undone.
          </p>
          <p class="text-(--ui-text-muted)">
            The original CSV file is not touched. Classifications already computed are
            kept and reused if the same courses are imported again. A large dataset can
            take a minute to delete.
          </p>
        </template>
        <p v-if="deleteDataset.error.value" class="text-(--ui-color-error-500)">
          Delete failed: {{ deleteDataset.error.value.message }}
        </p>
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="open = false">Cancel</UButton>
        <UButton color="error" :loading="deleteDataset.isPending.value" @click="onDelete">
          {{ incomplete ? "Finish Deleting" : "Delete Dataset" }}
        </UButton>
      </div>
    </template>
  </UModal>
</template>
