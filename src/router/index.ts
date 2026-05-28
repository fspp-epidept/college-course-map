import { createMemoryHistory, createRouter, type RouteRecordRaw } from "vue-router";

// The workbench drives navigation through the workspace store (active activity
// + open tabs), not URL routes — there's no URL bar in Tauri and tab state is
// richer than a path. The router stays wired so deep-linking can be added later
// (e.g., `/datasets/:id` opens that dataset's tab on launch) without re-plumbing.
// No routes yet — the workbench doesn't render any RouterView. Kept as an empty
// array so deep-linking (e.g., `/datasets/:id` opens that dataset) can be added
// later without re-introducing the router everywhere.
const routes: RouteRecordRaw[] = [];

export const router = createRouter({
  history: createMemoryHistory(),
  routes,
});
