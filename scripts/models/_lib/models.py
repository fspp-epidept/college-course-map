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


# The bare two-digit/four-digit/six-digit subdirs are what the app loads
# (inference.rs, models_install, check_parity.rs) — currently the RoBERTa
# exports. Other families publish to HF but are not app-active.
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
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-four-digit-ModernBERT-base",
        output_subdir="four-digit-modernbert",
        display_name="Four-digit CCM (ModernBERT)",
        digit_level=4,
        panel_label_column="inventory_cip_four",
        onnx_repo_slug="courses-four-digit-modernbert-base-onnx",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-six-digit-ModernBERT-base",
        output_subdir="six-digit-modernbert",
        display_name="Six-digit CCM (ModernBERT)",
        digit_level=6,
        panel_label_column="inventory_cip_six",
        onnx_repo_slug="courses-six-digit-modernbert-base-onnx",
    ),
)

# Subdirs the Rust side consumes as parity fixtures (see check_parity.rs,
# which bails on subdirs outside this set).
APP_ACTIVE_SUBDIRS = frozenset({"two-digit", "four-digit", "six-digit"})
