<script setup lang="ts">
import { storeToRefs } from "pinia";
import { useThemeStore } from "../../stores/theme";

const theme = useThemeStore();
// `storeToRefs` so the template re-renders on activeId / registry changes. Calling
// store.setTheme(id) is a non-reactive method call; everything else is reactive.
const { registry, activeId } = storeToRefs(theme);

function pick(id: string): void {
  // setTheme is async (writes settings.json via IPC) but we don't need to await
  // here — the store updates activeId optimistically inside applyById, so the UI
  // reflects the change immediately.
  void theme.setTheme(id);
}
</script>

<template>
  <section class="flex flex-col gap-5">
    <header>
      <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">Theme</h2>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Choose how the app looks. The active theme is persisted to
        <code class="text-xs">settings.json</code> and applied at startup.
      </p>
    </header>

    <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
      <button
        v-for="entry in registry"
        :key="entry.id"
        type="button"
        class="rounded-lg border p-4 text-left transition-colors"
        :class="
          entry.id === activeId
            ? 'border-(--ui-color-primary-500) bg-(--ui-bg-muted)'
            : 'border-(--ui-border) hover:bg-(--ui-bg-muted)'
        "
        @click="pick(entry.id)"
      >
        <div class="flex items-center justify-between">
          <span class="text-sm font-medium text-(--ui-text)">{{ entry.name }}</span>
          <UIcon
            v-if="entry.id === activeId"
            name="i-lucide-check"
            class="size-4 text-(--ui-color-primary-500)"
          />
        </div>
        <div class="mt-1 text-xs text-(--ui-text-muted) flex items-center gap-2">
          <span class="capitalize">{{ entry.colorScheme }}</span>
          <span aria-hidden="true">·</span>
          <span class="capitalize">{{ entry.variant }}</span>
        </div>
      </button>
    </div>
  </section>
</template>
