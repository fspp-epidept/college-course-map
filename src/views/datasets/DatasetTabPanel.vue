<script setup lang="ts">
import type { OpenTab } from "../../stores/workspace";

defineProps<{ tab: OpenTab }>();
</script>

<template>
  <div class="h-full p-6 flex flex-col gap-4">
    <header class="flex items-center gap-3">
      <UIcon name="i-lucide-database" class="size-5 text-(--ui-text-muted)" />
      <h2 class="text-lg font-medium">{{ tab.label }}</h2>
    </header>
    <p class="text-sm text-(--ui-text-muted)">
      Dataset detail will land here — schema preview, row counts, classification status.
    </p>
    <div class="rounded-lg border border-(--ui-border) p-6 bg-(--ui-bg-elevated) text-sm text-(--ui-text-dimmed)">
      Placeholder body for tab <code>{{ tab.id }}</code>.
    </div>
  </div>
</template>
