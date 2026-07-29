/**
 * Shared run-lifecycle vocabulary (EPI-97): one icon + semantic color +
 * label per state, used by the Runs sidebar rows, the run tab badge, and any
 * other surface that renders lifecycle. PRODUCT.md requires lifecycle to be
 * carried by iconography + text, never color alone — the icon shape differs
 * per state, so the pairing holds in monochrome themes too.
 */
export interface RunStateMeta {
  /** Lucide icon name. */
  icon: string;
  /** Classes for the standalone icon (semantic color; motion for running). */
  iconClass: string;
  /** Classes for a filled badge (tab panel). */
  badgeClass: string;
  /** Human label for the state. */
  label: string;
}

const META: Record<string, RunStateMeta> = {
  running: {
    icon: "i-lucide-loader-circle",
    iconClass: "text-(--ui-color-info-500) animate-spin motion-reduce:animate-none",
    badgeClass: "bg-(--ui-color-info-500)/15 text-(--ui-color-info-500)",
    label: "Running",
  },
  pending: {
    icon: "i-lucide-clock",
    iconClass: "text-(--ui-color-info-500)",
    badgeClass: "bg-(--ui-color-info-500)/15 text-(--ui-color-info-500)",
    label: "Pending",
  },
  interrupted: {
    icon: "i-lucide-pause",
    iconClass: "text-(--ui-color-warning-500)",
    badgeClass: "bg-(--ui-color-warning-500)/15 text-(--ui-color-warning-500)",
    label: "Paused",
  },
  completed: {
    icon: "i-lucide-check",
    iconClass: "text-(--ui-color-success-500)",
    badgeClass: "bg-(--ui-color-success-500)/15 text-(--ui-color-success-500)",
    label: "Completed",
  },
  failed: {
    icon: "i-lucide-x",
    iconClass: "text-(--ui-color-error-500)",
    badgeClass: "bg-(--ui-color-error-500)/15 text-(--ui-color-error-500)",
    label: "Failed",
  },
  cancelled: {
    icon: "i-lucide-ban",
    iconClass: "text-(--ui-text-dimmed)",
    badgeClass: "bg-(--ui-bg-muted) text-(--ui-text-muted)",
    label: "Cancelled",
  },
};

const UNKNOWN: RunStateMeta = {
  icon: "i-lucide-circle-help",
  iconClass: "text-(--ui-text-dimmed)",
  badgeClass: "bg-(--ui-bg-muted) text-(--ui-text-muted)",
  label: "Unknown",
};

export function runStateMeta(state: string | undefined): RunStateMeta {
  return (state !== undefined && META[state]) || UNKNOWN;
}
