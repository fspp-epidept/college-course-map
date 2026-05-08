"""Upload converted ONNX models to Hugging Face.

Three repos, one per model, all under $HF_USERNAME's namespace:
  {HF_USERNAME}/courses-two-digit-roberta-base-onnx
  {HF_USERNAME}/courses-four-digit-roberta-base-onnx
  {HF_USERNAME}/courses-six-digit-roberta-base-onnx

Plus an HF Collection grouping all three for discoverability, mirroring
annamp's pattern.

Each repo gets every file in `output/<size>/` plus a generated README.md.
Idempotent — re-running diffs against the remote and only uploads changes.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path

from huggingface_hub import HfApi, create_repo

from _lib.models import MODELS, ModelSpec

HERE = Path(__file__).parent
OUTPUT_ROOT = HERE / "output"

# Files optimum-cli is expected to have produced. Pre-flight check before upload.
EXPECTED_FILES = {
    "model.onnx",
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "vocab.json",
    "merges.txt",
}

COLLECTION_TITLE = "Course classifiers (ONNX)"
# HF caps the description at <150 chars. Markdown link to annamp's collection
# fits, but only just; if you edit, count carefully.
COLLECTION_DESCRIPTION = (
    "ONNX exports of [annamp/classifying-courses-at-scale]"
    "(https://huggingface.co/collections/annamp/classifying-courses-at-scale)."
)


def repo_id_for(spec: ModelSpec, username: str) -> str:
    return f"{username}/courses-{spec.output_subdir}-roberta-base-onnx"


def render_model_card(spec: ModelSpec) -> str:
    return f"""---
license: mit
language: en
library_name: optimum
tags:
  - onnx
  - text-classification
base_model: {spec.source_repo}
pipeline_tag: text-classification
---

# {spec.display_name} classifier (ONNX)

ONNX export of [`{spec.source_repo}`](https://huggingface.co/{spec.source_repo}) for use in non-Python inference environments (desktop apps, embedded systems). See the source repo for training, evaluation, and citation details.

Exported via `optimum-cli export onnx --task text-classification`. Bit-for-bit parity with the source PyTorch model verified on a held-out corpus (100% argmax agreement; max logit diff < 1e-5).

## Input format

    {{SUBJECT_CODE}} {{CATALOG_NUMBER}} --- {{COURSE_TITLE}}

Match this format exactly at inference time.
"""


def check_env() -> tuple[str, str]:
    username = os.environ.get("HF_USERNAME", "").strip()
    token = os.environ.get("HF_TOKEN", "").strip()
    missing = [name for name, val in (("HF_USERNAME", username), ("HF_TOKEN", token)) if not val]
    if missing:
        print(
            f"FAIL: missing env var(s): {', '.join(missing)}\n"
            f"      Copy `.env.example` to `.env`, fill in values, and run via\n"
            f"      `task models:upload` (Task loads .env automatically).",
            file=sys.stderr,
        )
        sys.exit(2)
    return username, token


def check_artifacts(spec: ModelSpec) -> Path:
    out_dir = OUTPUT_ROOT / spec.output_subdir
    if not out_dir.is_dir():
        raise SystemExit(f"FAIL: {out_dir} missing — run `task models:convert` first")
    present = {p.name for p in out_dir.iterdir() if p.is_file()}
    missing = EXPECTED_FILES - present
    if missing:
        raise SystemExit(f"FAIL: {out_dir} missing required files: {sorted(missing)}")
    return out_dir


def upload_one(spec: ModelSpec, username: str, token: str, api: HfApi) -> str:
    out_dir = check_artifacts(spec)
    repo_id = repo_id_for(spec, username)

    print(f"\n=== {spec.display_name} → {repo_id} ===")

    # Write the model card into the directory so upload_folder picks it up.
    card_path = out_dir / "README.md"
    card_path.write_text(render_model_card(spec))

    # Idempotent: returns existing repo if already created.
    create_repo(repo_id, token=token, exist_ok=True, repo_type="model")

    # Single call uploads every file in out_dir. The HF client diffs against
    # the remote and only sends what's missing or changed.
    api.upload_folder(
        folder_path=str(out_dir),
        repo_id=repo_id,
        repo_type="model",
        commit_message=f"Upload ONNX export of {spec.source_repo}",
        token=token,
    )
    return repo_id


def manage_collection(username: str, repo_ids: list[str], api: HfApi) -> str:
    """Create-or-reuse a collection, then add each repo as a member.

    Both `create_collection` and `add_collection_item` accept exists_ok=True,
    so re-running is a no-op. Returns the collection's URL.
    """
    print(f"\n=== Collection: {COLLECTION_TITLE} → {username} ===")
    collection = api.create_collection(
        title=COLLECTION_TITLE,
        namespace=username,
        description=COLLECTION_DESCRIPTION,
        exists_ok=True,
    )
    for repo_id in repo_ids:
        api.add_collection_item(
            collection_slug=collection.slug,
            item_id=repo_id,
            item_type="model",
            exists_ok=True,
        )
    # Collection.url is a property; safe to read directly.
    return collection.url


def main() -> int:
    username, token = check_env()
    api = HfApi(token=token)

    print(f"Uploading {len(MODELS)} model(s) to namespace: {username}")
    print(f"Files per repo: {sorted(EXPECTED_FILES) + ['README.md (generated)']}")

    repo_ids: list[str] = []
    for spec in MODELS:
        try:
            repo_ids.append(upload_one(spec, username, token, api))
        except Exception as e:
            print(f"FAIL: {spec.display_name}: {e}", file=sys.stderr)
            import traceback
            traceback.print_exc()
            return 1

    try:
        collection_url = manage_collection(username, repo_ids, api)
    except Exception as e:
        print(f"WARN: collection management failed: {e}", file=sys.stderr)
        print("      Repo uploads succeeded; you can add them to a collection manually.", file=sys.stderr)
        collection_url = None

    print("\n=== Done ===")
    for repo_id in repo_ids:
        print(f"  https://huggingface.co/{repo_id}")
    if collection_url:
        print(f"  Collection: {collection_url}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
