"""Validate the native planner evidence retained by a two-stage inference run."""

import hashlib
import json


def split_admissions_digest(value):
    stages = value.get("stages")
    if value.get("schema_version") != 2 or not isinstance(stages, list) or len(stages) != 2:
        raise ValueError("split evidence requires two native stage admissions")
    first, last = stages
    if (first.get("layer_start") != 0 or not isinstance(first.get("layer_end"), int)
            or first["layer_end"] <= 0 or last.get("layer_start") != first["layer_end"]
            or not isinstance(last.get("layer_end"), int) or last["layer_end"] <= last["layer_start"]
            or not first.get("package_id") or first.get("package_id") != last.get("package_id")):
        raise ValueError("split admissions disagree on package or contiguous layer ranges")
    for stage in stages:
        if not stage.get("plan_id") or not stage.get("resident_tensor_ids") or not stage.get("profiles"):
            raise ValueError("split admission is missing a native plan, tensor closure or profiles")
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()
