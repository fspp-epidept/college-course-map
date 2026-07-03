import { type MaybeRefOrGetter, computed, toValue } from "vue";
import { useQuery } from "@tanstack/vue-query";
import { type CoursePage, type CoverageRow, commands } from "../bindings";

interface UseCoursesArgs {
  datasetId: MaybeRefOrGetter<string>;
  modelId: MaybeRefOrGetter<number | null>;
  /**
   * Key-set cursor — the next page begins at the first row with
   * `row_index >= cursor`. `null` (and 0) load the first page.
   */
  cursor: MaybeRefOrGetter<number | null>;
  pageSize: MaybeRefOrGetter<number>;
  /**
   * When false, the query is disabled — useful while a dataset is still
   * importing so we don't pile up `list_courses_with_results` IPCs against a
   * DB that's getting hammered by the Appender on the writer side.
   */
  enabled?: MaybeRefOrGetter<boolean>;
}

/**
 * Server-paginated courses for a dataset, optionally joined against a model's
 * inference results. Reactive in all args — TanStack Query refetches when any
 * change. The query key includes the model id so switching digit level
 * doesn't surface stale joined columns from a different model.
 */
export function useCourses(args: UseCoursesArgs) {
  return useQuery({
    queryKey: computed(
      () =>
        [
          "courses",
          toValue(args.datasetId),
          toValue(args.modelId),
          toValue(args.cursor),
          toValue(args.pageSize),
        ] as const,
    ),
    queryFn: async (): Promise<CoursePage> => {
      const result = await commands.listCoursesWithResults({
        datasetId: toValue(args.datasetId),
        modelId: toValue(args.modelId),
        cursor: toValue(args.cursor),
        limit: toValue(args.pageSize),
      });
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
    enabled: computed(() => (args.enabled === undefined ? true : !!toValue(args.enabled))),
    // Keep prior page data visible while the next page loads so the table
    // doesn't flash empty on pagination clicks.
    placeholderData: (prev) => prev,
  });
}

/**
 * Per-model classification coverage for a dataset (EPI-68): one row per
 * manifest-active model with `classified` / `total` counts. Drives the
 * dataset tab's per-level coverage chips and the pre-run confirm panel's
 * "already classified" line. Refreshed via the global run-lifecycle watcher
 * (`useRunLifecycleRefresh`) rather than per-tick polling — three joined
 * COUNTs against a 2M-row dataset are not a 250 ms query.
 */
export function useCoverage(datasetId: MaybeRefOrGetter<string>) {
  return useQuery({
    queryKey: computed(() => ["coverage", toValue(datasetId)] as const),
    queryFn: async (): Promise<CoverageRow[]> => {
      const result = await commands.getClassificationCoverage(toValue(datasetId));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}

/**
 * Resolve the seeded `models.id` for a digit level. Used by the dataset view
 * to wire the courses join without forcing the frontend to know surrogate ids.
 */
export function useModelIdForDigitLevel(level: MaybeRefOrGetter<2 | 4 | 6>) {
  return useQuery({
    queryKey: computed(() => ["models", "digit-level", toValue(level)] as const),
    queryFn: async (): Promise<number | null> => {
      const result = await commands.modelIdForDigitLevel(toValue(level));
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}
