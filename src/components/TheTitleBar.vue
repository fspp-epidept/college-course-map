<script setup lang="ts">
import { useWindowControls } from "../composables/useWindowControls";

const { isMaximized, minimize, toggleMaximize, close } = useWindowControls();
</script>

<!--
  Custom window chrome for Windows/Linux (frameless window). Styling here is
  intentionally minimal/neutral — structure and behavior only. Restyle freely.
  `data-tauri-drag-region` marks the draggable area; the buttons are excluded
  because they don't carry the attribute.
-->
<template>
  <div
    data-tauri-drag-region
    class="titlebar"
    @dblclick="toggleMaximize"
  >
    <span data-tauri-drag-region class="titlebar__title">Course Classifier</span>

    <div class="titlebar__controls">
      <button type="button" class="titlebar__btn" aria-label="Minimize" @click="minimize">
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" />
        </svg>
      </button>
      <button
        type="button"
        class="titlebar__btn"
        :aria-label="isMaximized ? 'Restore' : 'Maximize'"
        @click="toggleMaximize"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" aria-hidden="true">
          <rect x="0.5" y="0.5" width="9" height="9" />
        </svg>
      </button>
      <button type="button" class="titlebar__btn titlebar__btn--close" aria-label="Close" @click="close">
        <svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor" aria-hidden="true">
          <line x1="0" y1="0" x2="10" y2="10" />
          <line x1="10" y1="0" x2="0" y2="10" />
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  height: 2rem;
  user-select: none;
  border-bottom: 1px solid var(--ui-border);
}

.titlebar__title {
  padding-inline: 0.75rem;
  font-size: 0.75rem;
  color: var(--ui-text-muted);
}

.titlebar__controls {
  margin-left: auto;
  display: flex;
  height: 100%;
}

.titlebar__btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 2.75rem;
  height: 100%;
  color: var(--ui-text-muted);
}

.titlebar__btn:hover {
  background: var(--ui-bg-accented);
  color: var(--ui-text);
}

.titlebar__btn--close:hover {
  background: #e81123;
  color: #fff;
}
</style>
