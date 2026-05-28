<script setup lang="ts">
// Quiet landing — no charts (handoff). Real content (recent runs/datasets,
// CTAs into Import/Models) lands when those features come online.
</script>

<template>
  <div class="h-full p-8 flex flex-col gap-6">
    <header>
      <h1 class="text-2xl font-semibold text-(--ui-text-highlighted)">
        College Course Map
      </h1>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Bulk-classify courses against CCM codes.
      </p>
    </header>

    <div class="grid gap-4 sm:grid-cols-3">
      <div class="rounded-lg border border-(--ui-border) p-5 bg-(--ui-bg-elevated)">
        <p class="text-xs uppercase tracking-wide text-(--ui-text-muted)">Datasets</p>
        <p class="mt-2 text-2xl font-medium">—</p>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">Import a CSV to get started.</p>
      </div>
      <div class="rounded-lg border border-(--ui-border) p-5 bg-(--ui-bg-elevated)">
        <p class="text-xs uppercase tracking-wide text-(--ui-text-muted)">Runs</p>
        <p class="mt-2 text-2xl font-medium">—</p>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">No classification runs yet.</p>
      </div>
      <div class="rounded-lg border border-(--ui-border) p-5 bg-(--ui-bg-elevated)">
        <p class="text-xs uppercase tracking-wide text-(--ui-text-muted)">Models</p>
        <p class="mt-2 text-2xl font-medium">—</p>
        <p class="mt-1 text-xs text-(--ui-text-dimmed)">2/4/6-digit RoBERTa.</p>
      </div>
    </div>
  </div>
</template>
