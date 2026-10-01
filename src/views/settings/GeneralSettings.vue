<script setup lang="ts">
// Real general settings (default model selection, data directory override,
// telemetry opt-out, etc.) land alongside their underlying features. The
// reset action moves to Settings → Storage once that section exists (#196).
import ResetAppData from "./ResetAppData.vue";
</script>

<template>
  <section class="flex flex-col gap-3">
    <header>
      <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">General</h2>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        App-wide preferences.
      </p>
    </header>
    <ResetAppData />
  </section>
</template>
