import type { BuiltinTheme } from "../types";
import { defaultDark } from "./default-dark";
import { defaultLight } from "./default-light";
import { highContrast } from "./high-contrast";

// Bundled themes, always available and the safe fallback set. One theme per file;
// register new built-ins here.
export const builtinThemes: BuiltinTheme[] = [defaultLight, defaultDark, highContrast];

/** The id applied when settings are missing/corrupt or a referenced theme can't
 *  be resolved. Must be a built-in so it can never fail to load. */
export const FALLBACK_THEME_ID = "default-light";
