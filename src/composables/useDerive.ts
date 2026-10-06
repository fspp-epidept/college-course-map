import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { type MaybeRefOrGetter, computed, toValue } from "vue";
import {
  type DerivePreview,
  type DeriveRequest,
  type DerivedColumns,
  type Derivation,
  commands,
} from "../bindings";

/**
 * The default output columns for a set of source datasets
 * (`derive.rs::derived_columns`): headers matched by key across sources.
 * Refetched when the sources change; the dialog re-seeds its drafts from it.
 */
export function useDerivedColumns(sources: MaybeRefOrGetter<string[]>) {
  return useQuery({
    queryKey: computed(() => ["derivedColumns", toValue(sources)] as const),
    queryFn: async (): Promise<DerivedColumns> => {
      const result = await commands.derivedColumns(toValue(sources));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    enabled: computed(() => toValue(sources).length > 0),
    staleTime: 60_000,
  });
}

/**
 * What a request would build (`derive.rs::preview_derivation`): match
 * counts per source and the first rows under the new headers. The request
 * is the key, so every change re-queries; the caller debounces it.
 */
export function useDerivePreview(
  request: MaybeRefOrGetter<DeriveRequest | null>,
  enabled: MaybeRefOrGetter<boolean>,
) {
  return useQuery({
    queryKey: computed(() => ["derivePreview", toValue(request)] as const),
    queryFn: async (): Promise<DerivePreview> => {
      const req = toValue(request);
      if (!req) throw new Error("no request");
      const result = await commands.previewDerivation(req);
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    enabled: computed(() => toValue(request) !== null && toValue(enabled)),
    placeholderData: (prev) => prev,
  });
}

/**
 * Start building a derived dataset. Returns as soon as the dataset row
 * exists; the datasets list shows it as importing until the copy is done.
 */
export function useCreateDerivedDataset() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (req: DeriveRequest): Promise<string> => {
      const result = await commands.createDerivedDataset(req);
      if (result.status === "error") throw new Error(result.error);
      return result.data.datasetId;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["datasets"] });
      queryClient.invalidateQueries({ queryKey: ["metrics"] });
    },
  });
}

/** How a derived dataset was built; `null` for a dataset that isn't one. */
export function useDerivation(
  datasetId: MaybeRefOrGetter<string>,
  enabled: MaybeRefOrGetter<boolean>,
) {
  return useQuery({
    queryKey: computed(() => ["derivation", toValue(datasetId)] as const),
    queryFn: async (): Promise<Derivation | null> => {
      const result = await commands.getDerivation(toValue(datasetId));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    enabled: computed(() => toValue(enabled)),
  });
}
