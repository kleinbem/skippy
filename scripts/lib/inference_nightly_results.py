"""Build the public nightly report from validated Actions artifacts."""

from pathlib import Path
import re
import sys

from .ci_artifacts import read_json
from .inference_ladder import matrix
from .inference_admission import split_admissions_digest
from .inference_nightly_metrics import METRIC_NAMES, WORKLOADS, contract, metric_value
from .inference_throughput import successful_run


def measurements(directory, entry, sha):
    receipt = read_json(directory / "measurements.json")
    job = read_json(directory / f'{entry["configuration"]}-job.json')
    run_id = successful_run(job, sha)
    expected = {
        "source_sha": sha, "configuration": entry["configuration"], "profile": entry["profile"],
        "job_id": job["id"], "run_id": run_id,
    }
    if (type(receipt.get("schema_version")) is not int or receipt["schema_version"] != 1
            or any(receipt.get(field) != value for field, value in expected.items())
            or not re.fullmatch(r"job_[A-Za-z0-9_]+", job["id"])):
        raise ValueError("inference measurement receipt identity mismatch")
    if entry["mode"] == "split":
        formation = read_json(directory / f'{entry["configuration"]}-formation.json')
        members = formation["members"]
        if (formation["id"] != job["formation_id"] or formation["status"] != "succeeded"
                or len(members) != 2 or {member["role"] for member in members} != {"leader", "worker"}
                or any(member["status"] != "succeeded" for member in members)
                or not any(member["role"] == "leader" and member["job_id"] == job["id"] for member in members)):
            raise ValueError("nightly receipt requires a successful split formation")
    if entry["mode"] == "split":
        if receipt.get("split_admissions_sha256") != split_admissions_digest(read_json(directory / "split-admissions.json")):
            raise ValueError("nightly split admission evidence mismatch")
    workload = WORKLOADS[entry["profile"]]
    model = entry["configuration"].removesuffix(f'-{entry["mode"]}')
    fingerprint = contract(entry["profile"], entry, model)
    if receipt["contract_sha256"] != fingerprint:
        raise ValueError("inference workload fingerprint mismatch")
    values = receipt["metrics"]
    expected_metrics = METRIC_NAMES[entry["profile"]]
    if set(values) != set(expected_metrics):
        raise ValueError("incomplete inference metric roster")
    if ("request_error_pct" in values and metric_value(values["request_error_pct"]) != 0):
        raise ValueError("AgentX request errors invalidate the performance measurements")
    return workload, [
        {"workload": workload, "metric": key, "contract_sha256": fingerprint,
         "measured": metric_value(values[key], positive=key != "request_error_pct")}
        for key in expected_metrics
    ]


def failure_outcome(directory, entry, sha):
    path = directory / f'{entry["configuration"]}-job.json'
    if path.is_file():
        try:
            job = read_json(path)
            if job.get("repo_rev") == sha:
                return {"failed": "failed", "cancelled": "cancelled",
                        "expired": "failed"}.get(job.get("status"), "incomplete")
        except (OSError, ValueError, TypeError, AttributeError):
            pass
    return "incomplete"


def read_configuration(root, entry, sha, run_id, attempt):
    directory = root / f'inference-{entry["id"]}-{run_id}-{attempt}'
    try:
        workload, rows = measurements(directory, entry, sha)
        return {"id": entry["configuration"], "workload": workload, "outcome": "passed", "metrics": rows}
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        print(f'{entry["id"]}: unverified nightly measurements ({type(error).__name__})', file=sys.stderr)
        workload = WORKLOADS[entry["profile"]]
        return {
            "id": entry["configuration"], "workload": workload,
            "outcome": failure_outcome(directory, entry, sha),
            "metrics": [{"workload": workload, "metric": metric, "measured": None}
                        for metric in METRIC_NAMES[entry["profile"]]],
        }


def nightly_run(root: Path, source, run_id, attempt, started_at, matrix_passed):
    configs = [
        read_configuration(root, entry, source, run_id, attempt)
        for entry in matrix("nightly", source, include_smoke=True)
    ]
    passed = matrix_passed and all(config["outcome"] == "passed" for config in configs)
    return {
        "id": f"{run_id}-{attempt}", "profile": "nightly", "source_sha": source,
        "started_at": started_at, "outcome": "passed" if passed else "failed",
        "published": True, "configurations": configs,
        "note": (
            "AgentX values are means of two trial summaries. Request counts are combined. "
            "The performance tolerance and improvement threshold are 5%."
        ),
    }
