<script setup lang="ts">
import { ref } from "vue";
import { commands } from "../../bindings";

// Reset app data (#206). Self-contained so it can move to Settings → Storage
// (#196) as one tag. Rust writes a marker and relaunches; the next startup
// deletes the data folders before anything opens them.
const confirmOpen = ref(false);
const resetSettings = ref(false);
const resetError = ref<string | null>(null);

async function reset(): Promise<void> {
  resetError.value = null;
  // Workspace selections point at datasets and runs that are about to go.
  localStorage.clear();
  const result = await commands.requestReset(!resetSettings.value);
  // Success never returns: the app relaunches.
  if (result.status === "error") resetError.value = result.error;
}
</script>

<template>
  <div class="flex items-center gap-3">
    <UButton size="xs" color="error" variant="outline" @click="confirmOpen = true">
      Reset app data…
    </UButton>
    <span class="text-xs text-(--ui-text-muted)">
      Deletes all datasets, runs, cached results, downloaded models and runtime packs.
    </span>
  </div>
  <p v-if="resetError" class="mt-2 text-sm text-(--ui-color-error-500)">
    Reset failed: {{ resetError }}
  </p>
  <UModal v-model:open="confirmOpen" title="Reset app data">
    <template #body>
      <div class="flex flex-col gap-3 text-sm">
        <p class="text-(--ui-text-muted)">
          The app relaunches and starts as on first run. All datasets, runs and
          cached classifications are deleted, and models and runtime packs must be
          downloaded again. This cannot be undone.
        </p>
        <UCheckbox
          v-model="resetSettings"
          label="Also reset settings and themes"
          description="Restores default compute settings and removes custom themes."
        />
      </div>
    </template>
    <template #footer>
      <div class="flex justify-end gap-2 w-full">
        <UButton variant="ghost" color="neutral" @click="confirmOpen = false">Cancel</UButton>
        <UButton color="error" @click="reset">Reset and relaunch</UButton>
      </div>
    </template>
  </UModal>
</template>
