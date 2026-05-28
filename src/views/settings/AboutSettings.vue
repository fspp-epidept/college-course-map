<script setup lang="ts">
import packageJson from "../../../package.json";

// `package.json` import is resolved by Vite at build time and tree-shaken to just
// what we read — no whole-manifest blob ships to the WebView.
const version: string = packageJson.version;
</script>

<template>
  <section class="flex flex-col gap-3">
    <header>
      <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">About</h2>
      <p class="mt-1 text-sm text-(--ui-text-muted)">
        Course Classifier — a native desktop app for bulk-classifying courses
        against CCM codes.
      </p>
    </header>
    <dl class="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
      <dt class="text-(--ui-text-muted)">Version</dt>
      <dd class="text-(--ui-text)">{{ version }}</dd>
    </dl>
  </section>
</template>
