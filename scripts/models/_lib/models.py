"""Single source of truth for the model specs.

Every script in this pipeline imports `MODELS` from here. Adding a new model
variant (e.g., a quantized precision tier) means appending one entry; nothing
else changes.
"""
from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class ModelSpec:
    source_repo: str            # HF repo to pull from
    output_subdir: str          # local folder under output/
    display_name: str           # for logs and reports
    digit_level: int            # 2, 4, or 6
    panel_label_column: str     # ground-truth column in validation.csv
    onnx_repo_slug: str         # repo name (sans namespace) the export publishes to
    # App-facing install dir (two-digit/four-digit/six-digit) when this spec is
    # the family the app ships; None for specs that only publish to HF. Drives
    # the manifest (manifest.py), the parity fixture (verify.py), and
    # models_install's copy mapping. Exactly one spec per digit level sets it.
    app_subdir: str | None = None


# The app-active family is ModernBERT (decision 2026-07-03, EPI-56): the specs
# with app_subdir set are what the manifest pins, what check_parity.rs
# verifies, and what installs into the app's two/four/six-digit model dirs.
# RoBERTa specs remain published on HF but are no longer app-active.
MODELS: tuple[ModelSpec, ...] = (
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-two-digit-roberta-base",
        output_subdir="two-digit",
        display_name="Two-digit CCM",
        digit_level=2,
        panel_label_column="inventory_cip_two",
        onnx_repo_slug="courses-two-digit-roberta-base-onnx",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-four-digit-roberta-base",
        output_subdir="four-digit",
        display_name="Four-digit CCM",
        digit_level=4,
        panel_label_column="inventory_cip_four",
        onnx_repo_slug="courses-four-digit-roberta-base-onnx",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-six-digit-roberta-base",
        output_subdir="six-digit",
        display_name="Six-digit CCM",
        digit_level=6,
        panel_label_column="inventory_cip_six",
        onnx_repo_slug="courses-six-digit-roberta-base-onnx",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-two-digit-ModernBERT-base",
        output_subdir="two-digit-modernbert",
        display_name="Two-digit CCM (ModernBERT)",
        digit_level=2,
        panel_label_column="inventory_cip_two",
        onnx_repo_slug="courses-two-digit-modernbert-base-onnx",
        app_subdir="two-digit",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-four-digit-ModernBERT-base",
        output_subdir="four-digit-modernbert",
        display_name="Four-digit CCM (ModernBERT)",
        digit_level=4,
        panel_label_column="inventory_cip_four",
        onnx_repo_slug="courses-four-digit-modernbert-base-onnx",
        app_subdir="four-digit",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-six-digit-ModernBERT-base",
        output_subdir="six-digit-modernbert",
        display_name="Six-digit CCM (ModernBERT)",
        digit_level=6,
        panel_label_column="inventory_cip_six",
        onnx_repo_slug="courses-six-digit-modernbert-base-onnx",
        app_subdir="six-digit",
    ),
)

APP_ACTIVE: tuple[ModelSpec, ...] = tuple(s for s in MODELS if s.app_subdir is not None)
