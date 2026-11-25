"""Summarize complete scored smoke receipts for the inference report."""

import math
import re
import time
from pathlib import Path

from .ci_artifacts import read_json, write_json
from .jy_qualification import PilotHTTPError

ARTIFACT_RETRIES = 6
ARTIFACT_RETRY_DELAY = 5


def fetch_artifact(pilot, run_id, filename):
    path = f"/v1/artifacts/{run_id}/raw/{filename}"
    # JY can report a successful run before its artifacts are available.
    for remaining in range(ARTIFACT_RETRIES, -1, -1):
        try:
            return pilot.api("GET", path)
        except PilotHTTPError as error:
            if error.status != 404 or error.path != path or remaining == 0:
                raise RuntimeError(f"{run_id}/{filename}: {error}") from error
            time.sleep(ARTIFACT_RETRY_DELAY)


def successful_run(job, sha):
    runs = job.get("runs") or []
    run = runs[-1] if runs else {}
    run_id = run.get("id") or ""
    if (job.get("repo_rev") != sha or job.get("status") != "succeeded"
            or run.get("status") != "succeeded" or not re.fullmatch(r"run_[A-Za-z0-9_]+", run_id)):
        raise ValueError("missing successful native attempt")
    return run_id


def collect(pilot, job, row, sha, output, key):
    run_id = successful_run(job, sha)
    qualification = fetch_artifact(pilot, run_id, "qualification.json")
    samples = fetch_artifact(pilot, run_id, "quality.json")
    identity = {"source_sha": sha, "model": key, "mode": row["mode"],
                "profile": "pr", "status": "passed"}
    if any(qualification.get(field) != expected for field, expected in identity.items()):
        raise ValueError("throughput evidence identity mismatch")
    cases = read_json(Path("ci/inference-quality.json"))
    if not isinstance(samples, list) or len(samples) != len(cases):
        raise ValueError("incomplete scored throughput evidence")
    prefill, decode = [], []
    for sample, case in zip(samples, cases):
        if sample.get("passed") is not True or sample.get("case") != case:
            raise ValueError("unscored or mismatched throughput sample")
        response = sample["response"]
        timing = response.get("timings") or {}
        cached = ((response.get("usage") or {}).get("prompt_tokens_details") or {}).get("cached_tokens")
        prompt = timing.get("prompt_n")
        tokens = prompt - cached if type(prompt) is int and type(cached) is int and cached >= 0 else None
        prefill.append((tokens, timing.get("prompt_ms")))
        decode.append((timing.get("predicted_n"), timing.get("predicted_ms")))
    p_tokens, p_ms = totals(prefill)
    d_tokens, d_ms = totals(decode)
    receipt = {
        "source_sha": sha, "configuration": row["id"], "job_id": job["id"], "run_id": run_id,
        "requests": len(samples), "prefill_tokens": p_tokens, "prefill_ms": p_ms,
        "decode_tokens": d_tokens, "decode_ms": d_ms,
    }
    write_json(output / f'{row["id"]}-throughput.json', receipt)
    return qualification, receipt


def totals(rows):
    tokens, duration = 0, 0.0
    for count, millis in rows:
        if (type(count) is not int or count <= 0 or type(millis) not in {float, int}
                or not math.isfinite(millis) or millis <= 0):
            return 0, 0.0
        tokens += count
        duration += millis
        if tokens >= 2**64 or not math.isfinite(duration):
            return 0, 0.0
    return tokens, duration
