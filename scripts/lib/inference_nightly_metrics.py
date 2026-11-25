"""Collect the publishable metrics from source-bound Spark receipts."""

import hashlib
import json
from math import isfinite
from pathlib import Path
from statistics import fmean
import time

from .ci_artifacts import digest, write_json
from .inference_inputs import fixture, input_hashes
from .inference_admission import split_admissions_digest
from .inference_throughput import collect, successful_run

WORKLOADS = {"pr": "DGX Spark smoke v1", "nightly": "DGX Spark AgentX nightly v1"}
SMOKE_METRICS = ("prefill_tok_s", "decode_tok_s")
AGENTX_METRICS = ("output_tok_s", "ttft_p95_ms", "itl_p95_ms", "requests", "request_error_pct")
METRIC_NAMES = {"pr": SMOKE_METRICS, "nightly": AGENTX_METRICS}
HARNESS = (
    "scripts/inference-execute.py", "scripts/lib/inference_checks.py",
    "scripts/lib/inference_nightly_metrics.py", "ci/inference-quality.json",
)


def metric_value(value, *, positive=False):
    if type(value) not in (float, int) or not isfinite(value) or value < 0 or (positive and not value):
        raise ValueError("invalid inference measurement")
    return value


def await_artifacts(read):
    for attempt in range(13):
        try:
            return read()
        except RuntimeError as error:
            if "JY HTTP 404:" not in str(error) or attempt == 12:
                raise
            time.sleep(5)


def smoke_metrics(receipt):
    def rate(phase):
        tokens = metric_value(receipt[f"{phase}_tokens"], positive=True)
        millis = metric_value(receipt[f"{phase}_ms"], positive=True)
        return metric_value(tokens * 1000 / millis, positive=True)

    return {name: rate(phase) for name, phase in zip(SMOKE_METRICS, ("prefill", "decode"))}


def agentx_metrics(trials):
    if len(trials) != 2:
        raise ValueError("nightly AgentX requires exactly two trials")
    requests = []
    series = {key: [] for key in AGENTX_METRICS[:3]}
    for trial in trials:
        if (trial["aiperf_version"] != "0.13.0" or trial["is_complete"] is not True
                or trial["metadata"]["submission_valid"] is not True or trial["error_summary"] != []
                or metric_value(trial["request_error_rate"]["avg"]) != 0):
            raise ValueError("invalid or unsuccessful AgentX trial")
        count = metric_value(trial["request_count"]["avg"], positive=True)
        if int(count) != count:
            raise ValueError("AgentX request count is not an integer")
        requests.append(int(count))
        series["output_tok_s"].append(metric_value(trial["output_token_throughput"]["avg"], positive=True))
        series["ttft_p95_ms"].append(metric_value(trial["time_to_first_token"]["p95"], positive=True))
        series["itl_p95_ms"].append(metric_value(trial["inter_token_latency"]["p95"], positive=True))
    return {**{key: fmean(values) for key, values in series.items()},
            "requests": sum(requests), "request_error_pct": 0}


def contract(profile, row, key):
    files = HARNESS
    if profile == "pr":
        files += ("scripts/lib/inference_throughput.py",)
    else:
        files += ("ci/inference-corpus.json", "ci/aiperf-requirements.lock")
    inputs = {
        "profile": profile, "mode": row["mode"], "device": "NVIDIA GB10",
        "admission": "native-stage-plan-v2", "model": fixture(key),
        "harness": {name: digest(Path(name)) for name in files},
    }
    encoded = json.dumps(inputs, sort_keys=True, separators=(",", ":")).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def collect_nightly(pilot, job, row, sha, output, key, profile):
    if profile not in METRIC_NAMES:
        raise ValueError("nightly publication only supports AgentX and smoke evidence")
    run_id = successful_run(job, sha)
    prefix = f"/v1/artifacts/{run_id}"
    if profile == "pr":
        qualification, receipt = await_artifacts(lambda: collect(pilot, job, row, sha, output, key))
        measurements = smoke_metrics(receipt)
    else:
        qualification = await_artifacts(lambda: pilot.api("GET", prefix + "/raw/qualification.json"))
        trials = [
            await_artifacts(lambda: pilot.api("GET", f"{prefix}/profile/agentx-profile-{index}.json"))
            for index in range(2)
        ]
        measurements = agentx_metrics(trials)
    expected = {"source_sha": sha, "model": key, "mode": row["mode"], "profile": profile,
                "status": "passed", "artifact_manifest_sha256": input_hashes()["model_manifest_sha256"]}
    if any(qualification.get(field) != value for field, value in expected.items()):
        raise ValueError("nightly measurement provenance mismatch")
    if qualification.get("gpu", "").strip() != "NVIDIA GB10":
        raise ValueError("nightly evidence requires the DGX Spark GPU")
    admission_hash = None
    if row["mode"] == "split":
        admissions = await_artifacts(lambda: pilot.api("GET", prefix + "/raw/split-admissions.json"))
        admission_hash = split_admissions_digest(admissions)
        if qualification.get("split_admissions_sha256") != admission_hash:
            raise ValueError("nightly split admission evidence mismatch")
        write_json(output / "split-admissions.json", admissions)
    write_json(output / "measurements.json", {
        "schema_version": 1, "source_sha": sha, "configuration": row["id"], "profile": profile,
        "job_id": job["id"], "run_id": run_id, "split_admissions_sha256": admission_hash,
        "contract_sha256": contract(profile, row, key), "metrics": measurements,
    })
