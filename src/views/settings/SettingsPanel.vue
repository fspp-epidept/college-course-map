<script setup lang="ts">
import { settingsSections } from "../../config/settingsSections";
</script>

<template>
  <!-- One scrolling page (EPI-95): every section renders stacked with
       dividers; the sidebar entries are scroll anchors, not panel swaps.
       Section ids match `settings-<section.id>` for the sidebar's
       scrollIntoView. -->
  <div class="h-full overflow-y-auto px-8 py-6">
    <div class="flex flex-col gap-8 max-w-3xl">
      <template v-for="(section, index) in settingsSections" :key="section.id">
        <USeparator v-if="index > 0" />
        <div :id="`settings-${section.id}`" class="scroll-mt-4">
          <component :is="section.component" />
        </div>
      </template>
    </div>
  </div>
</template>
