<script setup lang="ts">
import { computed } from "vue";
import type { Finding, InputProfile } from "../bindings";
import { INPUT_FINDINGS } from "../config/inputFindings";
import InputSamples from "./InputSamples.vue";

// Renders an input profile (src-tauri/src/profile.rs): the sample model
// inputs and the findings that crossed their thresholds. Used by the import
// dialog on the dry-run profile and later by the dataset detail on the
// persisted one. `compact` keeps only each finding's label, title, and count.
const props = defineProps<{
  profile: InputProfile;
  compact?: boolean;
}>();

const warnings = computed(() => props.profile.findings.filter((f) => f.severity === "warning"));

function n(value: number): string {
  return value.toLocaleString();
}

function rateLabel(rate: number): string {
  const pct = rate * 100;
  return pct > 0 && pct < 0.1 ? "<0.1%" : `${pct.toFixed(1)}%`;
}

function countLine(f: Finding): string {
  if (f.rate === 0 && f.examples.count === 0) {
    return `${n(f.count)} distinct values`;
  }
  return `${n(f.count)} of ${n(props.profile.importable)} rows (${rateLabel(f.rate)})`;
}
</script>

<template>
  <div class="flex flex-col gap-3 text-sm">
    <div class="flex flex-col gap-1">
      <span class="font-medium text-(--ui-text)">How rows are sent to the model</span>
      <InputSamples :samples="profile.samples" />
    </div>

    <p v-if="profile.findings.length === 0" class="text-(--ui-text-muted)">
      No input issues found in {{ n(profile.importable) }} rows.
    </p>
    <div v-else class="flex flex-col gap-3">
      <p class="text-xs text-(--ui-text-dimmed)">
        {{ n(warnings.length) }} {{ warnings.length === 1 ? "warning" : "warnings" }},
        {{ n(profile.findings.length - warnings.length) }}
        {{ profile.findings.length - warnings.length === 1 ? "note" : "notes" }}
      </p>
      <div v-for="f in profile.findings" :key="f.code" class="flex flex-col gap-0.5">
        <div class="flex items-baseline gap-2">
          <span
            class="text-[10px] uppercase tracking-wide font-medium"
            :class="
              f.severity === 'warning' ? 'text-(--ui-color-warning-500)' : 'text-(--ui-text-dimmed)'
            "
          >
            {{ f.severity === "warning" ? "Warning" : "Note" }}
          </span>
          <span class="font-medium text-(--ui-text)">{{ INPUT_FINDINGS[f.code].title }}</span>
          <span class="text-xs text-(--ui-text-dimmed)">{{ countLine(f) }}</span>
        </div>
        <template v-if="!compact">
          <p class="text-xs text-(--ui-text-muted)">
            {{ INPUT_FINDINGS[f.code].explanation }} {{ INPUT_FINDINGS[f.code].remedy }}
          </p>
          <ul v-if="f.examples.first.length > 0" class="font-mono text-xs text-(--ui-text-dimmed)">
            <li v-for="e in f.examples.first" :key="e.row" class="truncate">
              row {{ n(e.row) }}: {{ e.input }}
            </li>
          </ul>
        </template>
      </div>
    </div>
  </div>
</template>
