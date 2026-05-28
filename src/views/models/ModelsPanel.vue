<script setup lang="ts">
// Fixed-kind activity: one full panel, no tabs. Model management UI lands here
// when #25 / model-fetch wiring comes online.
</script>

<template>
  <div class="h-full p-8 flex flex-col gap-4">
    <header>
      <h1 class="text-2xl font-semibold text-(--ui-text-highlighted)">Models</h1>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Bundled CCM RoBERTa models. Download, verify, and pick a default per run.
      </p>
    </header>
    <div class="rounded-lg border border-(--ui-border) p-6 bg-(--ui-bg-elevated) text-sm text-(--ui-text-dimmed)">
      Model status, sha256 verification, and HF fetch UI land here.
    </div>
  </div>
</template>
