import { useQuery } from "@tanstack/vue-query";
import { type AppMetrics, commands } from "../bindings";

/**
 * Landing-screen aggregates. Anything that changes a row count (an import,
 * a classification ending, a dataset delete) should invalidate `["metrics"]`.
 */
export function useMetrics() {
  return useQuery({
    queryKey: ["metrics"],
    queryFn: async (): Promise<AppMetrics> => {
      const result = await commands.listMetrics();
      if (result.status === "error") throw new Error(result.error);
      return result.data;
    },
  });
}
