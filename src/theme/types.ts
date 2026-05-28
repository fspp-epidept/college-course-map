// Theme token shapes are generated from the Rust structs (see #58 / store.rs);
// re-export them here so the frontend has one import site for theme types.
export type {
  ColorRamp,
  ColorRamps,
  ColorScheme,
  SemanticTokens,
  Theme,
  ThemeSummary,
} from "../bindings";

import type { ColorScheme, Theme } from "../bindings";

/** Where a theme came from. Built-ins ship in the bundle and are the always-safe
 *  fallback; `user` themes are read from `themes/*.json` in the config dir. */
export type ThemeVariant = "official" | "user";

/** A built-in theme. The generated `Theme` type omits `id` (it's the filename
 *  stem for user themes); built-ins declare their own. */
export type BuiltinTheme = Theme & { id: string };

/** A registry row: enough to list/select a theme without its token payload. */
export interface RegistryEntry {
  id: string;
  name: string;
  colorScheme: ColorScheme;
  variant: ThemeVariant;
}
