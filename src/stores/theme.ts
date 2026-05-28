import { acceptHMRUpdate, defineStore } from "pinia";
import { computed, ref } from "vue";
import { commands, type Theme } from "../bindings";
import { FALLBACK_THEME_ID, builtinThemes } from "../theme/builtins";
import type { RegistryEntry } from "../theme/types";

// Pinia setup store — global client state lives here, matching the workspace
// store pattern (this used to be a composable singleton with module-scoped
// refs; folded into Pinia to keep one shape for app-global state).

const builtinEntries: RegistryEntry[] = builtinThemes.map((theme) => ({
  id: theme.id,
  name: theme.name,
  colorScheme: theme.colorScheme,
  variant: "official",
}));

// CSS custom properties set by the last-applied theme, cleared before the next so
// switching themes never leaves stale tokens behind. Module-scoped because it's
// pure DOM bookkeeping, not reactive state — no benefit to lifting it into pinia.
let appliedProps: string[] = [];

/** Apply a theme's tokens to `<html>` via inert `setProperty`, set the font, and
 *  toggle the `.dark` class that Nuxt UI's `--ui-*` overrides key off. */
function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  for (const prop of appliedProps) root.style.removeProperty(prop);
  appliedProps = [];

  const set = (prop: string, value: string): void => {
    root.style.setProperty(prop, value);
    appliedProps.push(prop);
  };

  if (theme.tokens) {
    // Semantic token key -> `--ui-<key>` (e.g. `bg-muted` -> `--ui-bg-muted`).
    for (const [key, value] of Object.entries(theme.tokens)) {
      if (value) set(`--ui-${key}`, value);
    }
  }
  if (theme.colors) {
    // Role ramp -> `--ui-color-<role>-<shade>` (e.g. `--ui-color-primary-500`).
    for (const [role, ramp] of Object.entries(theme.colors)) {
      if (!ramp) continue;
      for (const [shade, value] of Object.entries(ramp)) {
        if (value) set(`--ui-color-${role}-${shade}`, value);
      }
    }
  }

  root.style.fontFamily = theme.font ?? "";
  root.classList.toggle("dark", theme.colorScheme === "dark");
}

/** Resolve a theme's full token payload: built-ins from the bundle, user themes
 *  from Rust. Returns null if a user theme can't be read. */
async function resolveTheme(id: string): Promise<Theme | null> {
  const builtin = builtinThemes.find((theme) => theme.id === id);
  if (builtin) return builtin;
  const result = await commands.readTheme(id);
  return result.status === "ok" ? result.data : null;
}

export const useThemeStore = defineStore("theme", () => {
  const userEntries = ref<RegistryEntry[]>([]);
  const activeId = ref<string>(FALLBACK_THEME_ID);

  /** Built-in + user themes, built-ins first. */
  const registry = computed<RegistryEntry[]>(() => [...builtinEntries, ...userEntries.value]);

  const activeTheme = computed<RegistryEntry | null>(
    () => registry.value.find((entry) => entry.id === activeId.value) ?? null,
  );

  /** Apply a theme by id, tracking it as active. Returns false if it couldn't be
   *  resolved (caller decides whether to fall back). Exposed so `bootstrapTheme`
   *  can run it before mount without also persisting through `setTheme`. */
  async function applyById(id: string): Promise<boolean> {
    const theme = await resolveTheme(id);
    if (!theme) return false;
    applyTheme(theme);
    activeId.value = id;
    return true;
  }

  /** Reload the user-theme registry from the config dir. */
  async function refreshUserThemes(): Promise<void> {
    const result = await commands.listThemes();
    userEntries.value =
      result.status === "ok"
        ? result.data.map((summary) => ({ ...summary, variant: "user" as const }))
        : [];
  }

  /** Switch the active theme and persist the choice. No-op if it can't be applied. */
  async function setTheme(id: string): Promise<void> {
    if (!(await applyById(id))) return;
    await commands.writeSettings({ activeTheme: id });
  }

  return {
    registry,
    activeId,
    activeTheme,
    applyById,
    refreshUserThemes,
    setTheme,
  };
});

/**
 * One-shot load before `app.mount()` (see main.ts): populate the registry, read
 * the active theme from settings, and apply it — so first paint is already themed
 * (no FOUC). Pinia must be installed on the app before this is called so the
 * store resolves. Always succeeds: a missing/corrupt setting or unreadable theme
 * falls back to the built-in default.
 */
export async function bootstrapTheme(): Promise<void> {
  const store = useThemeStore();
  await store.refreshUserThemes();
  const settings = await commands.readSettings();
  const wantedId = settings.status === "ok" ? settings.data.activeTheme : FALLBACK_THEME_ID;
  if (!(await store.applyById(wantedId))) {
    await store.applyById(FALLBACK_THEME_ID);
  }
}

if (import.meta.hot) {
  import.meta.hot.accept(acceptHMRUpdate(useThemeStore, import.meta.hot));
}
