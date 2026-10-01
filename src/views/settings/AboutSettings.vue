<script setup lang="ts">
import { ref } from "vue";
import { commands } from "../../bindings";
import packageJson from "../../../package.json";

// `package.json` import is resolved by Vite at build time and tree-shaken to just
// what we read — no whole-manifest blob ships to the WebView.
const version: string = packageJson.version;

// Diagnostic log (EPI-109): what to attach to a bug report. Rust opens the
// folder so the WebView needs no opener permission.
async function openLogs(): Promise<void> {
  await commands.openLogsDir();
}

// The bundled NCES report the CCM taxonomy comes from.
const reportError = ref<string | null>(null);
async function openReport(): Promise<void> {
  const result = await commands.openCcmReference();
  reportError.value = result.status === "error" ? result.error : null;
}
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
      <dt class="text-(--ui-text-muted)">Logs</dt>
      <dd class="flex items-center gap-3">
        <UButton size="xs" variant="outline" @click="openLogs">Open logs folder</UButton>
        <span class="text-xs text-(--ui-text-muted)">
          Attach app.log to a bug report — it holds diagnostics only, no course data.
        </span>
      </dd>
      <dt class="text-(--ui-text-muted)">CCM reference</dt>
      <dd class="flex flex-col gap-1">
        <span class="flex items-center gap-3">
          <UButton size="xs" variant="outline" @click="openReport">Open 2010 College Course Map</UButton>
          <span v-if="reportError" class="text-xs text-(--ui-color-error-500)">{{ reportError }}</span>
        </span>
        <span class="text-xs text-(--ui-text-muted)">
          Bryan, M. &amp; Simone, S. (2012). 2010 College Course Map (NCES 2012-162rev). National
          Center for Education Statistics, Institute of Education Sciences, U.S. Department of
          Education. Washington, DC. Public domain.
        </span>
      </dd>
    </dl>
  </section>
</template>
