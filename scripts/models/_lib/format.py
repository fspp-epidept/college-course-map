"""Canonical model-input formatter.

Format B per annamp's model card:
    https://huggingface.co/annamp/classifying-courses-at-scale-two-digit-roberta-base

The Tauri Rust app must produce byte-identical strings. The JSON spec emitted
by `export_spec()` is the contract — Rust reads that file at build time and
asserts its assembler matches.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any

FORMAT_VERSION = "v1"
TEMPLATE = "{subject_code} {catalog_number} --- {course_title}"


@dataclass(frozen=True)
class CourseInput:
    subject_code: str
    catalog_number: str
    course_title: str


def format_input(course: CourseInput) -> str:
    return TEMPLATE.format(
        subject_code=course.subject_code,
        catalog_number=course.catalog_number,
        course_title=course.course_title,
    )


def export_spec() -> dict[str, Any]:
    return {
        "version": FORMAT_VERSION,
        "template": TEMPLATE,
        "fields": ["subject_code", "catalog_number", "course_title"],
        "source": "annamp model card (verified 2026-05)",
    }


if __name__ == "__main__":
    out = Path(__file__).parent / "format_spec.json"
    out.write_text(json.dumps(export_spec(), indent=2) + "\n")
    print(f"Wrote {out}")
