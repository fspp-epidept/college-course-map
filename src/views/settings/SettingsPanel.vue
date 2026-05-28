<script setup lang="ts">
// Settings panel — fixed kind, single view. Until the picker (#108) lands the
// theme is switched by editing ~/.config/college-course-map/settings.json.
</script>

<template>
  <div class="h-full p-8 flex flex-col gap-4">
    <header>
      <h1 class="text-2xl font-semibold text-(--ui-text-highlighted)">Settings</h1>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Pick a theme, manage models, configure runs.
      </p>
    </header>
    <div class="rounded-lg border border-(--ui-border) p-6 bg-(--ui-bg-elevated) text-sm text-(--ui-text-dimmed)">
      Sectioned settings (general / theme / about) land here. Theme picker tracked
      in #108.
    </div>
  </div>
</template>
