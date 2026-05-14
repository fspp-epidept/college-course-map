# Issue labels

Issues are tagged along three orthogonal axes. Pick one from each that applies (Area + Phase always; Type only when it fits). Combining all three gives the kanban / Project filter view its grouping.

Per-label hover descriptions live on the labels themselves in GitHub's UI; this file explains the *taxonomy*.

## Area — which part of the stack

Where the work lands.

| Label              | Meaning                                        |
| ------------------ | ---------------------------------------------- |
| `area:vue`         | Vue / frontend code under `src/`               |
| `area:rust`        | Rust / `src-tauri/` code                       |
| `area:python`      | Python model-conversion pipeline (`scripts/`)  |
| `area:schema`      | DuckDB schema / DDL                            |
| `area:tauri`       | Tauri infra: config, capabilities, bundle      |
| `area:docs`        | Documentation                                  |
| `area:ci`          | CI / repo tooling (Biome, clippy, Taskfile, GH Actions) |
| `area:security`    | Security hardening (CSP, capabilities, URL/CSV escaping) |
| `area:cross-cutting` | Spans multiple areas; usually a decision record |

## Phase — handoff phases retained from the original build order

Phases 1–11 mirror the build order in `docs/handoff.md` (retired doc) and `tmp/project-seed.md`. They're stable groupings of *when* the work most naturally lands, not strict prerequisites. Cross-cutting items aren't ordered.

| Label                  | Meaning                                                  |
| ---------------------- | -------------------------------------------------------- |
| `phase:1-scaffold`     | Scaffold + dev tooling (Tauri, Vue, Vite, Biome, clippy) |
| `phase:2-inference`    | Rust ONNX inference spike + Python↔Rust parity           |
| `phase:3-database`     | DuckDB schema + CSV ingest                               |
| `phase:4-pipeline`     | Cache-aware inference pipeline (tokio channels, resume)  |
| `phase:5-concurrency`  | DuckDB concurrency validation (stress test)              |
| `phase:6-models-ui`    | Model management UI (HF download, selection)             |
| `phase:7-import`       | Import flow (IPC commands + mapping configurator)        |
| `phase:8-mvp`          | MVP loop: import → classify → browse                     |
| `phase:9-refresh`      | Source-file refresh + deletion detection                 |
| `phase:10-dashboard`   | Dashboard aggregations + search                          |
| `phase:11-distribution`| CSP, capabilities tightening, signing, bundles           |
| `phase:cross-cutting`  | CI, license, versioning, security baseline, decisions    |

## Type — optional, for issues whose shape matters

Most issues don't need a Type label. Use these when the shape of the work differs from a normal implementation task.

| Label            | Meaning                                                   |
| ---------------- | --------------------------------------------------------- |
| `type:decision`  | Captures a decided architectural / tooling choice — closes when the decision is recorded, not when the implementation lands |
| `type:spike`     | Time-boxed investigation; success = a written outcome, not necessarily merged code |

## Status

Status is managed by issue **open / closed** state, plus the eventual GitHub Project's `Status` column (Todo / In Progress / In Review / Done). We deliberately don't have `status:*` labels — they duplicate the project field and tend to drift from reality.

## Adding a new label

1. Update `tmp/seed-issues.sh`'s `mklbl` block so the label survives a re-seed.
2. Run `gh label create '<name>' --color <hex> --description '<text>' --repo fspp-epidept/college-course-map`.
3. Update this file's table.

If you find yourself reaching for the same fourth axis repeatedly, that's a signal to add a column to the project (a custom field) rather than a new label dimension. Labels are for the issue-list view; project fields are for the kanban / table view.
