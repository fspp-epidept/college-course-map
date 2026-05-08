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


MODELS: tuple[ModelSpec, ...] = (
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-two-digit-roberta-base",
        output_subdir="two-digit",
        display_name="Two-digit CIP",
        digit_level=2,
        panel_label_column="inventory_cip_two",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-four-digit-roberta-base",
        output_subdir="four-digit",
        display_name="Four-digit CIP",
        digit_level=4,
        panel_label_column="inventory_cip_four",
    ),
    ModelSpec(
        source_repo="annamp/classifying-courses-at-scale-six-digit-roberta-base",
        output_subdir="six-digit",
        display_name="Six-digit CIP",
        digit_level=6,
        panel_label_column="inventory_cip_six",
    ),
)
