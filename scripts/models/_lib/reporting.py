"""Markdown report writers."""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Any


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def render_parity_report(results: list[dict[str, Any]]) -> str:
    lines = [
        "# Parity report",
        "",
        f"Generated: {utc_now_iso()}",
        "",
        "Compares ONNX exports against PyTorch sources on a small synthetic corpus.",
        "Argmax should be ≥99% in most cases (occasional dips on near-tie inputs are",
        "acceptable). Max logit diff should be < 1e-3.",
        "",
        "| Model | n | Argmax | Top-3 | Max diff | Mean diff | Pass |",
        "|---|---:|---:|---:|---:|---:|:---:|",
    ]
    for r in results:
        lines.append(
            f"| {r['display_name']} | {r['n_inputs']} "
            f"| {r['argmax_agreement']:.1%} | {r['top3_agreement']:.1%} "
            f"| {r['max_logit_diff']:.2e} | {r['mean_logit_diff']:.2e} "
            f"| {'✓' if r['passed'] else '✗'} |"
        )
    return "\n".join(lines) + "\n"


def render_validation_report(results: list[dict[str, Any]], meta: dict[str, Any]) -> str:
    preferred = ", ".join(meta.get("preferred_providers", []))
    lines = [
        "# Validation report",
        "",
        f"Generated: {utc_now_iso()}",
        "",
        f"- Source CSV: `{meta['source_csv']}`",
        f"- Sample mode: `{meta['sample_mode']}` ({meta['sample_size']:,} rows)",
        f"- Input format: {meta['format']}",
        f"- Execution provider (actual): {meta['execution_provider']}",
        f"- Preferred order: {preferred}",
        f"- Filter: {meta['filter']}",
        "",
        "**Train/test split status: unknown.** Until annamp confirms whether this",
        "panel was held out from training, treat the accuracy numbers as preliminary.",
        "If the panel overlaps with the training set, accuracy will be inflated by",
        "memorization. The numbers are still useful as a sanity check that conversion",
        "didn't break the model.",
        "",
        "Accuracy compares predictions to `inventory_cip_*` after canonicalizing both",
        "to a common numeric form (panel uses bare digits, models use dotted CCM form;",
        "both are converted to floats for equality).",
        "",
        "| Model | Unique inputs | Rows compared | Accuracy | p50 (ms) | p95 (ms) |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for r in results:
        lines.append(
            f"| {r['display_name']} | {r['n_unique']:,} | {r['n_rows']:,} "
            f"| {r['accuracy']:.1%} "
            f"| {r['latency_p50_ms']:.2f} | {r['latency_p95_ms']:.2f} |"
        )
    lines.append("")
    lines.append("Disagreement detail (predicted ≠ panel label) lives in the run's")
    lines.append("`disagreements.csv`. See `output/validation/<run-id>/`.")
    return "\n".join(lines) + "\n"
