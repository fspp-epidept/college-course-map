import { useQuery } from "@tanstack/vue-query";
import { type AppMetrics, commands } from "../bindings";

/**
 * Landing-screen aggregates. Mutations that change a row count (import_csv,
 * start_run, dataset delete) should invalidate `["metrics"]` after success.
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
