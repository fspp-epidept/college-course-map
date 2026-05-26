import { createMemoryHistory, createRouter, type RouteRecordRaw } from "vue-router";

const routes: RouteRecordRaw[] = [
  { path: "/", redirect: "/datasets" },
  {
    path: "/datasets",
    name: "datasets",
    component: () => import("../views/DatasetsView.vue"),
  },
  {
    path: "/runs",
    name: "runs",
    component: () => import("../views/RunsView.vue"),
  },
  {
    path: "/models",
    name: "models",
    component: () => import("../views/ModelsView.vue"),
  },
  {
    path: "/settings",
    name: "settings",
    component: () => import("../views/SettingsView.vue"),
  },
];

export const router = createRouter({
  history: createMemoryHistory(),
  routes,
});
