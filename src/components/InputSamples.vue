<script setup lang="ts">
import type { Sample } from "../bindings";

// The one renderer for assembled model-input samples (profile.rs reservoir),
// shared by the import dialog, the dataset's input check, and the pre-run
// confirm panel, so "what the model reads" always looks the same.
defineProps<{ samples: Sample[] }>();
</script>

<template>
  <ul v-if="samples.length > 0" class="font-mono text-xs text-(--ui-text-muted)">
    <li v-for="s in samples" :key="s.row" class="truncate">{{ s.input }}</li>
  </ul>
  <p v-else class="text-xs text-(--ui-text-dimmed)">No importable rows to show.</p>
</template>
