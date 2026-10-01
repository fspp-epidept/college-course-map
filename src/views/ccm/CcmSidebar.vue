<script setup lang="ts">
import { computed, reactive, ref } from "vue";
import { type CcmSearchFields, searchCcm, useCcmTaxonomy } from "../../composables/useCcmTaxonomy";
import { useWorkspace } from "../../stores/workspace";

// Rendering every match as a button gets heavy past a few hundred rows; a
// broad query ("course") matches nearly all 2,167 entries.
const RESULT_LIMIT = 200;

const workspace = useWorkspace();
const { data: entries, isPending, isError, error } = useCcmTaxonomy();

const query = ref("");
const fields = reactive<CcmSearchFields>({ code: true, title: true, description: true });

// No query: browse the 48 two-digit families; a family's detail lists its codes.
const families = computed(() => (entries.value ?? []).filter((e) => e.digitLevel === 2));
const matches = computed(() => searchCcm(entries.value ?? [], query.value, fields));
const searching = computed(() => query.value.trim() !== "");
const shown = computed(() =>
  (searching.value ? matches.value : families.value).slice(0, RESULT_LIMIT),
);
</script>

<template>
  <div class="flex flex-col gap-2 p-2">
    <UInput
      v-model="query"
      icon="i-lucide-search"
      placeholder="Code, title, or description"
      aria-label="Search the CCM taxonomy"
    />
    <div class="flex gap-3 px-1 text-xs">
      <UCheckbox v-model="fields.code" label="Code" />
      <UCheckbox v-model="fields.title" label="Title" />
      <UCheckbox v-model="fields.description" label="Description" />
    </div>

    <p v-if="isPending" class="px-2 py-1.5 text-sm text-(--ui-text-dimmed)">Loading taxonomy…</p>

    <p v-else-if="isError" class="px-2 py-1.5 text-sm text-(--ui-color-error-500)">
      Failed to load taxonomy: {{ error?.message }}
    </p>

    <template v-else>
      <p v-if="searching" class="px-2 text-xs text-(--ui-text-dimmed) tabular-nums">
        {{ matches.length.toLocaleString() }} {{ matches.length === 1 ? "match" : "matches" }}
        <template v-if="matches.length > RESULT_LIMIT">
          — showing the first {{ RESULT_LIMIT }}; refine the search
        </template>
      </p>
      <div class="flex flex-col gap-0.5">
        <button
          v-for="entry in shown"
          :key="entry.code"
          type="button"
          class="text-left rounded px-2 py-1.5 flex gap-2"
          :class="
            workspace.selectedCcmCode === entry.code
              ? 'bg-(--ui-bg-accented)'
              : 'hover:bg-(--ui-bg-muted)'
          "
          :aria-current="workspace.selectedCcmCode === entry.code ? 'true' : undefined"
          @click="workspace.selectCcmCode(entry.code)"
        >
          <span class="text-xs text-(--ui-text-dimmed) tabular-nums shrink-0 pt-0.5">
            {{ entry.code }}
          </span>
          <span class="text-sm text-(--ui-text) line-clamp-2">{{ entry.title }}</span>
        </button>
      </div>
    </template>
  </div>
</template>
