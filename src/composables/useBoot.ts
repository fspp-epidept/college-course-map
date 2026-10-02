import {
  type ComputedRef,
  computed,
  type InjectionKey,
  inject,
  onBeforeUnmount,
  onMounted,
  provide,
  type Ref,
  ref,
  watch,
} from "vue";
import { type BootState, commands, events } from "../bindings";

/** How long startup runs before the boot screen shows, and the least time it stays once shown. */
const SCREEN_DELAY_MS = 500;

export interface Boot {
  /** The backend's boot state; `null` until the first snapshot or event. */
  state: Ref<BootState | null>;
  /** Whether the boot screen is up: startup has run past the delay, or failed. */
  screenVisible: Ref<boolean>;
  /** Whether the workbench may mount: ready, and the screen (if shown) has had its minimum time. */
  workbenchReady: ComputedRef<boolean>;
}

const bootKey: InjectionKey<Boot> = Symbol("boot");

/**
 * Own the boot state for the app (#224). Call once, in `App.vue`; children
 * read it with `useBoot()`. Subscribes to `bootStateChanged` before reading
 * the `boot_state` snapshot, and keeps whichever is newer by `seq`.
 */
export function provideBoot(): Boot {
  const state = ref<BootState | null>(null);
  const screenVisible = ref(false);
  const minimumShown = ref(false);
  const timers: ReturnType<typeof setTimeout>[] = [];
  let unlisten: (() => void) | undefined;

  function accept(next: BootState): void {
    if (state.value === null || next.seq > state.value.seq) state.value = next;
  }

  const status = computed(() => state.value?.status.status);

  onMounted(async () => {
    unlisten = await events.bootStateChanged.listen(({ payload }) => accept(payload.state));
    const snapshot = await commands.bootState();
    if (snapshot.status === "ok") accept(snapshot.data);
  });

  timers.push(
    setTimeout(() => {
      if (status.value !== "ready") screenVisible.value = true;
    }, SCREEN_DELAY_MS),
  );

  watch(status, (now) => {
    if (now === "failed") screenVisible.value = true;
  });

  watch(screenVisible, (shown) => {
    if (shown) timers.push(setTimeout(() => (minimumShown.value = true), SCREEN_DELAY_MS));
  });

  onBeforeUnmount(() => {
    unlisten?.();
    for (const timer of timers) clearTimeout(timer);
  });

  const boot: Boot = {
    state,
    screenVisible,
    workbenchReady: computed(
      () => status.value === "ready" && (!screenVisible.value || minimumShown.value),
    ),
  };
  provide(bootKey, boot);
  return boot;
}

/** The boot state `App.vue` provides. */
export function useBoot(): Boot {
  const boot = inject(bootKey);
  if (!boot) throw new Error("useBoot() needs provideBoot() in an ancestor");
  return boot;
}
