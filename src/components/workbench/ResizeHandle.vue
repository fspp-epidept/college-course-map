<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Track teardown of any in-flight drag so component unmount mid-drag (e.g. the
// sidebar gets hidden) doesn't leak listeners or leave the page locked.
let dragCleanup: (() => void) | null = null;
const dragging = ref(false);

function pixelsPerRem(): number {
  return Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

function onPointerDown(event: PointerEvent): void {
  // Only drag with the primary (left) button.
  if (event.button !== 0) return;
  event.preventDefault();

  const startX = event.clientX;
  const startRem = workspace.sidebarWidthRem;
  const ppr = pixelsPerRem();

  // Suppress text selection + force the resize cursor over the whole viewport
  // so the cursor doesn't snap back when the pointer leaves the handle's hit box.
  const prevUserSelect = document.body.style.userSelect;
  const prevCursor = document.body.style.cursor;
  document.body.style.userSelect = "none";
  document.body.style.cursor = "col-resize";
  dragging.value = true;

  function onMove(moveEvent: PointerEvent): void {
    const dxRem = (moveEvent.clientX - startX) / ppr;
    workspace.setSidebarWidth(startRem + dxRem);
  }

  function onUp(): void {
    cleanup();
  }

  function cleanup(): void {
    document.removeEventListener("pointermove", onMove);
    document.removeEventListener("pointerup", onUp);
    document.body.style.userSelect = prevUserSelect;
    document.body.style.cursor = prevCursor;
    dragging.value = false;
    dragCleanup = null;
  }

  document.addEventListener("pointermove", onMove);
  document.addEventListener("pointerup", onUp);
  dragCleanup = cleanup;
}

onBeforeUnmount(() => {
  dragCleanup?.();
});
</script>

<template>
  <!-- 4px hit area, fills vertical space, sits between sidebar and panel.
       Subtle until hovered/dragged; uses the theme's primary token so it
       remains visible across user themes. -->
  <div
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize primary sidebar"
    class="w-1 shrink-0 cursor-col-resize bg-transparent transition-colors hover:bg-(--ui-primary)/40"
    :class="{ 'bg-(--ui-primary)/60': dragging }"
    @pointerdown="onPointerDown"
  />
</template>
