# Product

<!-- Personality, anti-references, and the a11y target below were inferred from
     repo context on 2026-07-03 (init interview timed out). Register, users, and
     purpose come straight from CLAUDE.md. Edit anything that reads wrong. -->

## Register

product

## Users

Two personas, one machine:

- **The administrator** — university administrators and education-policy staff
  (UMich FSPP Education Policy Initiative), mostly non-technical, working on
  their own desktop machines. Their job: take an institutional course-inventory
  CSV (realistically ~2M rows / 200 MB), classify every course against CCM
  codes, and get trustworthy, browsable, exportable results — without a server,
  IT support, or a Python environment. Sessions are long: a classification run
  can outlast a coffee break, a lunch, or a reboot, and the user must be able
  to walk away and come back.
- **The researcher** — uses the outputs for research and papers. Needs clear,
  well-defined data and easy processing: unambiguous provenance (which model,
  which dataset, what coverage), honest per-model/per-dataset classification
  coverage visible at a glance, and exports clean enough to feed an analysis
  pipeline without archaeology.

## Product Purpose

A native desktop app (Tauri 2) that bulk-classifies courses against CCM codes
using local ONNX RoBERTa/ModernBERT models. Success looks like: an administrator
imports a 200 MB CSV, maps columns, starts a run, closes the lid, and later
resumes to a complete, cached, resumable result set they can filter and export
— never wondering whether the app is stuck, and never re-paying for inference
already done.

## Brand Personality

Precise, steady, unobtrusive. The lineage is a well-made workbench tool
(VS Code-shaped shell, dense tables, command palette) tempered for
non-technical users: plain language, honest state, no jargon walls. Trust is
earned through precision — exact counts, real progress, truthful lifecycle
states (`paused`, `failed`, `interrupted`) — not through decoration or
cheerfulness.

## Anti-references

- **Generic SaaS analytics dashboard**: hero metrics, identical card grids,
  gradient accents, decorative charts. This is a working tool, not a pitch deck.
- **Enterprise-gray sludge**: the lifeless admin panel where nothing has
  emphasis and every screen looks identical. Density must not mean deadness.
- **Consumer-app flash**: celebratory motion, mascots, playful gradients —
  anything that undermines "serious data infrastructure."

## Design Principles

1. **The tool disappears into the task.** Use earned, familiar affordances
   (workbench nav, sidebar master/detail, tables, palette) — never invent controls for standard
   jobs. An admin fluent in Excel and a browser should never pause at a widget.
2. **Long jobs deserve calm.** State of a 2M-row run is always legible:
   what's running, how far along, what happens if I close this. Progress is
   truthful, interruption is safe, resumption is obvious. No ambiguous spinners.
3. **Density is respect.** Administrators live in tables. Prefer information-
   forward layouts over whitespace theater; aggregation and slicing happen in
   the backend, and the UI shows exactly what was asked for.
4. **Honest states over reassurance.** Errors, partial results, and stale
   caches are shown as what they are, in plain language, with the next action
   attached. Never paper over a failed run.
5. **Neutral foundations, themeable skin.** The runtime theme system
   (`--ui-*` tokens) is the styling surface and the user drives the visual
   look. Build components against semantic tokens so every theme — including
   high-contrast — works without per-component fixes.

## Accessibility & Inclusion

WCAG 2.1 AA. Concretely:

- Body text ≥4.5:1 contrast in every built-in theme; the shipped
  `high-contrast` theme is maintained, not vestigial.
- Color is never the sole channel for classification status, run lifecycle, or
  confidence — pair with text or iconography.
- Full keyboard operability per the three-layer model in `docs/keybinds.md`.
- All motion respects `prefers-reduced-motion`; nothing blocks on animation.
