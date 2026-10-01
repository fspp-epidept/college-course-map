import { useQuery } from "@tanstack/vue-query";
import { type CcmEntry, commands } from "../bindings";

/**
 * The full CCM taxonomy (2,167 static rows) via `list_ccm_taxonomy`. Fetched
 * once per session — the table only changes through migrations.
 */
export function useCcmTaxonomy() {
  return useQuery({
    queryKey: ["ccm-taxonomy"],
    queryFn: async (): Promise<CcmEntry[]> => {
      const result = await commands.listCcmTaxonomy();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export interface CcmSearchFields {
  code: boolean;
  title: boolean;
  description: boolean;
}

/** The 2-digit family of any code ("26.0101" → "26"). Same slicing as the
 *  4→2-digit parent join in courses.rs / export.rs. */
export function ccmFamily(code: string): string {
  return code.slice(0, 2);
}

/** The 4-digit group prefix of a 6-digit code ("26.0101" → "26.01"). */
export function ccmGroup(code: string): string {
  return code.slice(0, 5);
}

// Fold case and the PDF's typographic punctuation so "women's" matches
// "Women’s" and "stage-craft" matches an en dash.
function fold(text: string): string {
  return text
    .toLowerCase()
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, '"')
    .replace(/[–—]/g, "-")
    .replace(/…/g, "...");
}

const CODE_TERM = /^[\d.]+$/;

/**
 * Entries matching every whitespace-separated term. A numeric term ("26.01",
 * "2601") matches code prefixes, dots ignored on both sides; any other term
 * matches a substring of the enabled text fields.
 */
export function searchCcm(
  entries: readonly CcmEntry[],
  query: string,
  fields: CcmSearchFields,
): CcmEntry[] {
  const terms = fold(query).split(/\s+/).filter(Boolean);
  if (terms.length === 0) return [];
  return entries.filter((entry) => {
    const code = entry.code.replace(/\./g, "");
    const text = fold(
      [fields.title ? entry.title : "", fields.description ? (entry.description ?? "") : ""].join(
        "\n",
      ),
    );
    return terms.every((term) =>
      CODE_TERM.test(term)
        ? fields.code && code.startsWith(term.replace(/\./g, ""))
        : text.includes(term),
    );
  });
}
