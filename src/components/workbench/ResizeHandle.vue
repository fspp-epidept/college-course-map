<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";
import { useWorkspace } from "../../stores/workspace";

const workspace = useWorkspace();

// Track teardown of any in-flight drag so an unmount mid-drag (e.g. the sidebar
// is hidden by clicking the active activity icon) doesn't leak listeners or
// leave the page locked.
let dragCleanup: (() => void) | null = null;
const dragging = ref(false);

function pixelsPerRem(): number {
  return Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

// Mouse events (not pointer events) — WebKitGTK's pointer-event surface has
// historically been flaky inside Tauri's webview; mouse events work uniformly.
function onMouseDown(event: MouseEvent): void {
  if (event.button !== 0) return;
  event.preventDefault();

  const startX = event.clientX;
  const startRem = workspace.sidebarWidthRem;
  const ppr = pixelsPerRem();

  // Suppress text selection + lock the resize cursor over the whole viewport
  // so it doesn't snap back when the mouse exits the 4px hit box.
  const prevUserSelect = document.body.style.userSelect;
  const prevCursor = document.body.style.cursor;
  document.body.style.userSelect = "none";
  document.body.style.cursor = "col-resize";
  dragging.value = true;

  function onMove(moveEvent: MouseEvent): void {
    const dxRem = (moveEvent.clientX - startX) / ppr;
    workspace.setSidebarWidth(startRem + dxRem);
  }

  function cleanup(): void {
    document.removeEventListener("mousemove", onMove);
    document.removeEventListener("mouseup", cleanup);
    document.body.style.userSelect = prevUserSelect;
    document.body.style.cursor = prevCursor;
    dragging.value = false;
    dragCleanup = null;
  }

  document.addEventListener("mousemove", onMove);
  document.addEventListener("mouseup", cleanup);
  dragCleanup = cleanup;
}

onBeforeUnmount(() => {
  dragCleanup?.();
});
</script>

<template>
  <!-- 4px wide, always-visible track using the theme border token so it reads
       as a deliberate seam, not invisible. Hovering and dragging tint it with
       the primary token. Cursor change is the click-affordance. -->
  <div
    role="separator"
    aria-orientation="vertical"
    aria-label="Resize primary sidebar"
    class="w-1 shrink-0 cursor-col-resize transition-colors"
    :class="
      dragging
        ? 'bg-(--ui-primary)'
        : 'bg-(--ui-border) hover:bg-(--ui-primary)'
    "
    @mousedown="onMouseDown"
  />
</template>
