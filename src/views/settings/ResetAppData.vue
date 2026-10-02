<script setup lang="ts">
import { useMutation } from "@tanstack/vue-query";
import { ref } from "vue";
import { commands } from "../../bindings";

// Reset app data (#206). Self-contained so it can move to Settings → Storage
// (#196) as one tag. Rust writes a marker; the relaunch then clears the data
// folders before anything opens them.
const confirmOpen = ref(false);
const resetSettings = ref(false);

const reset = useMutation({
  mutationFn: async () => {
    const result = await commands.requestReset(!resetSettings.value);
    if (result.status === "error") throw new Error(result.error);
    // Only once the marker is written: workspace selections point at
    // datasets and runs that are about to go.
    localStorage.clear();
    await commands.relaunchApp();
  },
});
</script>

<template>
  <div class="flex items-center gap-3">
    <UButton size="xs" color="error" variant="outline" class="shrink-0" @click="confirmOpen = true">
      Reset app data…
    </UButton>
    <span class="text-xs text-(--ui-text-muted)">
      Deletes all datasets, runs, cached results, downloaded models and runtime packs.
    </span>
  </div>
  <UModal v-model:open="confirmOpen" title="Reset app data">
    <template #body>
      <div class="flex flex-col gap-3 text-sm">
        <p class="text-(--ui-text-muted)">
          The app relaunches and starts as on first run. All datasets, runs and
          cached classifications are deleted, along with downloaded models and
          runtime packs. A run in progress is stopped. This cannot be undone.
        </p>
        <UCheckbox
          v-model="resetSettings"
          label="Also reset settings and themes"
          description="Restores default compute settings and removes custom themes."
        />
        <p v-if="reset.error.value" class="text-(--ui-color-error-500)">
          Reset failed: {{ reset.error.value.message }}
        </p>
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="confirmOpen = false">Cancel</UButton>
        <UButton color="error" :loading="reset.isPending.value" @click="reset.mutate()">
          Reset and relaunch
        </UButton>
      </div>
    </template>
  </UModal>
</template>
