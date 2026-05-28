import type { BuiltinTheme } from "../types";

// PLACEHOLDER PALETTE. These few overrides only exist to prove the token applier
// works end-to-end (switching to this theme visibly repaints background/text/
// borders/accent). The real high-contrast palette — full ramps, WCAG-checked
// contrast — is the user's to define. Replace these values; don't treat them as
// a finished a11y theme.
export const highContrast: BuiltinTheme = {
  id: "high-contrast",
  name: "High Contrast",
  colorScheme: "dark",
  tokens: {
    bg: "#000000",
    "bg-muted": "#000000",
    text: "#ffffff",
    "text-muted": "#ffffff",
    border: "#ffffff",
    primary: "#ffff00",
  },
};
