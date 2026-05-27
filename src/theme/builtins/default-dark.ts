import type { BuiltinTheme } from "../types";

// Empty token map: Nuxt UI's stock dark palette (the `.dark` defaults in the
// bundled CSS). Selecting it just toggles the `.dark` class via the applier.
export const defaultDark: BuiltinTheme = {
  id: "default-dark",
  name: "Default Dark",
  colorScheme: "dark",
};
