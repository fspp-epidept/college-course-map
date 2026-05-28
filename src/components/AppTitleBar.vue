<script setup lang="ts">
import { useWindowControls } from "../composables/useWindowControls";
import { useWorkspace } from "../stores/workspace";

const { isMaximized, minimize, toggleMaximize, close } = useWindowControls();
const workspace = useWorkspace();
</script>

<!--
  Custom window chrome for Windows/Linux (frameless window). Styling here is
  intentionally minimal/neutral — structure and behavior only. Restyle freely.
  `data-tauri-drag-region` marks the draggable area; interactive children
  (buttons) intercept clicks normally.
-->
<template>
  <div
    data-tauri-drag-region
    class="titlebar"
    @dblclick="toggleMaximize"
  >
    <span data-tauri-drag-region class="titlebar__title">Course Classifier</span>

    <!-- VS Code-style centered command-palette trigger. Absolute positioning
         keeps it true-center regardless of title/controls widths. -->
    <button
      type="button"
      class="titlebar__palette"
      aria-label="Open command palette"
      @click="workspace.toggleCommandPalette()"
    >
      <svg width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
        <circle cx="7" cy="7" r="5" />
        <line x1="14" y1="14" x2="10.5" y2="10.5" />
      </svg>
      <span class="titlebar__palette-label">Search</span>
      <span class="titlebar__palette-kbd">Ctrl K</span>
    </button>

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
  position: relative;
  display: flex;
  align-items: center;
  height: 2rem;
  user-select: none;
  border-bottom: 1px solid var(--ui-border);
  background: var(--ui-bg-elevated);
}

.titlebar__title {
  padding-inline: 0.75rem;
  font-size: 0.75rem;
  color: var(--ui-text-muted);
}

.titlebar__palette {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  height: 1.5rem;
  min-width: 18rem;
  max-width: 32rem;
  padding-inline: 0.625rem;
  border-radius: 0.375rem;
  background: var(--ui-bg);
  color: var(--ui-text-muted);
  font-size: 0.75rem;
  border: 1px solid var(--ui-border);
}

.titlebar__palette:hover {
  background: var(--ui-bg-accented);
  color: var(--ui-text);
}

.titlebar__palette-label {
  flex: 1;
  text-align: left;
}

.titlebar__palette-kbd {
  font-family: var(--font-mono, ui-monospace);
  font-size: 0.6875rem;
  padding: 0.0625rem 0.375rem;
  border-radius: 0.1875rem;
  background: var(--ui-bg-muted);
  color: var(--ui-text-dimmed);
  border: 1px solid var(--ui-border);
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
