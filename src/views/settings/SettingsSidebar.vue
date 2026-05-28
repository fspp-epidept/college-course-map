<script setup lang="ts">
// Settings sidebar lists sections; the picked section is local component state
// in the SettingsPanel until there are real sub-routes worth deep-linking.
</script>

<template>
  <div class="flex flex-col gap-1 p-2">
    <p class="px-2 text-xs uppercase tracking-wide text-(--ui-text-dimmed)">
      Sections
    </p>
    <div class="rounded px-2 py-1.5 text-sm text-(--ui-text) bg-(--ui-bg-muted)">General</div>
    <div class="rounded px-2 py-1.5 text-sm text-(--ui-text-muted) hover:bg-(--ui-bg-muted)">Theme</div>
    <div class="rounded px-2 py-1.5 text-sm text-(--ui-text-muted) hover:bg-(--ui-bg-muted)">About</div>
  </div>
</template>
