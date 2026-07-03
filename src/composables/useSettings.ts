import { type Settings, commands } from "../bindings";

/**
 * Read-modify-write for `settings.json`. `writeSettings` takes the whole
 * struct, and any field missing from the payload is reset to its serde
 * default on the Rust side — so every writer MUST go through this helper
 * with a patch instead of constructing a partial Settings object (that's
 * how a theme switch would silently reset the EP priority list, EPI-73).
 */
export async function patchSettings(patch: Partial<Settings>): Promise<Settings> {
  const current = await commands.readSettings();
  if (current.status === "error") throw new Error(current.error);
  const next = { ...current.data, ...patch };
  const written = await commands.writeSettings(next);
  if (written.status === "error") throw new Error(written.error);
  return next;
}
