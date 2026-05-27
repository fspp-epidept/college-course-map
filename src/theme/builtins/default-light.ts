import type { BuiltinTheme } from "../types";

// Empty token map: this theme is Nuxt UI's stock light palette (the `:root`
// defaults already in the bundled CSS). It exists so "Default Light" is a
// selectable, always-present entry and the safe fallback target.
export const defaultLight: BuiltinTheme = {
  id: "default-light",
  name: "Default Light",
  colorScheme: "light",
};
