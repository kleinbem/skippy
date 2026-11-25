"""Define the source-bound inference qualification matrix and split admission policy."""

import re
from pathlib import Path

from .ci_artifacts import read_json

LADDER = Path(__file__).resolve().parents[2] / "ci/inference-ladder.json"


def plan(profile, sha):
    if profile not in {"pr", "nightly", "release"}:
        raise ValueError("unknown inference profile")
    if not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
        raise ValueError("source must be an immutable 40-digit commit SHA")
    ladder = read_json(LADDER)
    if set(ladder) != {"schema_version", "models"} or type(ladder["schema_version"]) is not int:
        raise ValueError("invalid inference ladder schema")
    models = ladder["models"]
    if ladder["schema_version"] != 1 or not isinstance(models, list) or not models:
        raise ValueError("unsupported or empty inference ladder")
    checks = (["gpu-startup", "scored-quality", "streaming-smoke"] if profile == "pr"
              else ["correctness", "scored-quality", "agentx-performance"])
    rows, ids, names = [], set(), set()
    for model in models:
        if set(model) != {"id", "model", "architecture", "pr", "split"}:
            raise ValueError("invalid inference model fields")
        key, name, architecture = model["id"], model["model"], model["architecture"]
        if (
            not isinstance(key, str) or not re.fullmatch(r"[a-z0-9-]+", key) or key in ids
            or not isinstance(name, str) or not name.startswith("Qwen/") or name in names
            or architecture not in {"hybrid-dense", "full-attention-moe", "hybrid-moe", "qwen4-preview"}
            or type(model["pr"]) is not bool or type(model["split"]) is not bool
        ):
            raise ValueError(f"invalid or duplicate inference model: {key}")
        ids.add(key)
        names.add(name)
        if profile != "pr" or model["pr"]:
            for mode, machines in (("standalone", 1), ("split", 2)):
                if mode == "standalone" or model["split"]:
                    rows.append({"id": f"{key}-{mode}", "model": name, "architecture": architecture,
                                 "mode": mode, "machines": machines, "checks": checks})
    if not rows:
        raise ValueError("profile selects no inference models")
    return {
        "schema_version": 1, "status": "planned-not-executed", "profile": profile,
        "source_sha": sha, "hardware": "nvidia-gb10-dgx-spark",
        "execution": "jy-native-job-or-atomic-formation-outside-actions-container",
        "performance_claim": ("smoke-only" if profile == "pr"
                              else "extended-qualification-requires-pinned-harness-and-repeats"),
        "rows": rows,
    }


def matrix(profile, sha, include_smoke=False):
    rows = [{**row, "configuration": row["id"], "profile": profile}
            for row in plan(profile, sha)["rows"]]
    if include_smoke:
        if profile != "nightly":
            raise ValueError("the additional smoke matrix requires the nightly profile")
        rows.extend(
            {**row, "id": f'smoke-{row["id"]}', "configuration": row["id"], "profile": "pr"}
            for row in plan("pr", sha)["rows"]
        )
    return rows
