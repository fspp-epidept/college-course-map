<script setup lang="ts">
import { computed } from "vue";
import type { CcmEntry } from "../../bindings";
import { ccmFamily, ccmGroup, useCcmTaxonomy } from "../../composables/useCcmTaxonomy";
import { useWorkspace } from "../../stores/workspace";

const props = defineProps<{ code: string }>();

const workspace = useWorkspace();
const { data: entries } = useCcmTaxonomy();

const entry = computed(() => entries.value?.find((e) => e.code === props.code));
const family = computed(() =>
  entries.value?.find((e) => e.digitLevel === 2 && e.code === ccmFamily(props.code)),
);

// A family lists all its 6-digit codes; a 6-digit code lists the others in its
// 4-digit group (the taxonomy publishes no 4-digit titles).
const related = computed<CcmEntry[]>(() => {
  const current = entry.value;
  if (!current || !entries.value) return [];
  const prefix = current.digitLevel === 2 ? `${current.code}.` : ccmGroup(current.code);
  return entries.value.filter(
    (e) => e.digitLevel === 6 && e.code.startsWith(prefix) && e.code !== current.code,
  );
});
const relatedHeading = computed(() =>
  entry.value?.digitLevel === 2
    ? `Codes in this family (${related.value.length})`
    : `Other codes in group ${ccmGroup(props.code)}`,
);
</script>

<template>
  <div v-if="entries && !entry" class="p-4 text-sm text-(--ui-text-dimmed)">
    Code {{ code }} is not in the CCM taxonomy.
  </div>
  <article v-else-if="entry" class="p-4 flex flex-col gap-4 max-w-3xl">
    <header class="flex flex-col gap-1">
      <span class="text-sm text-(--ui-text-dimmed) tabular-nums">
        {{ entry.digitLevel }}-digit · {{ entry.code }}
      </span>
      <h2 class="text-xl font-semibold text-(--ui-text-highlighted)">{{ entry.title }}</h2>
      <span
        v-if="entry.titleShort && entry.titleShort !== entry.title"
        class="text-sm text-(--ui-text-muted)"
      >
        Short title: {{ entry.titleShort }}
      </span>
    </header>

    <p v-if="entry.description" class="text-sm text-(--ui-text) leading-relaxed">
      {{ entry.description }}
    </p>

    <p v-if="entry.digitLevel === 6 && family" class="text-sm">
      <span class="text-(--ui-text-muted)">Family: </span>
      <button
        type="button"
        class="text-(--ui-text) underline underline-offset-2 hover:text-(--ui-text-highlighted)"
        @click="workspace.selectCcmCode(family.code)"
      >
        {{ family.code }} {{ family.title }}
      </button>
    </p>

    <section v-if="related.length > 0" class="flex flex-col gap-1">
      <h3 class="text-sm font-medium text-(--ui-text-muted)">{{ relatedHeading }}</h3>
      <button
        v-for="r in related"
        :key="r.code"
        type="button"
        class="text-left rounded px-2 py-1 flex gap-3 hover:bg-(--ui-bg-muted)"
        @click="workspace.selectCcmCode(r.code)"
      >
        <span class="text-xs text-(--ui-text-dimmed) tabular-nums shrink-0 pt-0.5">{{ r.code }}</span>
        <span class="text-sm text-(--ui-text)">{{ r.title }}</span>
      </button>
    </section>
  </article>
</template>
