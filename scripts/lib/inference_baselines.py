"""Evaluate nightly measurements against saved, workload-compatible baselines."""

from copy import deepcopy
from decimal import Decimal
import re

from .inference_nightly_metrics import metric_value

TOLERANCE = Decimal("0.05")
DIRECTION = {
    "prefill_tok_s": 1, "decode_tok_s": 1, "output_tok_s": 1,
    "ttft_p95_ms": -1, "itl_p95_ms": -1,
}
SHA = re.compile(r"[0-9a-f]{40}")
CONTRACT = re.compile(r"[0-9a-f]{64}")


def metric_key(configuration, item):
    return configuration, item["workload"], item["metric"]


def baseline_index(baselines):
    result = {}
    for item in baselines:
        key = metric_key(item["configuration"], item)
        if (item["metric"] not in DIRECTION or key in result
                or not SHA.fullmatch(item["source_sha"])
                or not CONTRACT.fullmatch(item["contract_sha256"])):
            raise ValueError("invalid or duplicate saved inference baseline")
        metric_value(item["value"], positive=True)
        result[key] = item
    return result


def classify(metric, measured, saved):
    current, baseline = Decimal(str(measured)), Decimal(str(saved))
    signed = DIRECTION[metric] * (current - baseline)
    budget = baseline * TOLERANCE
    if signed < -budget:
        return "regressed"
    if signed > budget:
        return "improved"
    return "passed"


def apply_baselines(run, baselines):
    result = deepcopy(run)
    active = baseline_index(deepcopy(baselines))
    regression = False
    for config in result["configurations"]:
        if config["outcome"] != "passed":
            continue
        for row in config["metrics"]:
            metric = row["metric"]
            if metric not in DIRECTION:
                continue
            current = metric_value(row["measured"], positive=True)
            contract = row["contract_sha256"]
            if not CONTRACT.fullmatch(contract):
                raise ValueError("invalid inference workload fingerprint")
            key = metric_key(config["id"], row)
            saved = active.get(key)
            latest = {"value": current, "source_sha": result["source_sha"], "run_id": result["id"]}
            if saved is None or saved["contract_sha256"] != contract:
                row["baseline_update"] = "seeded"
            else:
                row["reference"] = {"value": saved["value"], "source_sha": saved["source_sha"]}
                verdict = classify(metric, current, saved["value"])
                row["verdict"] = verdict
                if verdict == "regressed":
                    regression = True
                    continue
                if verdict == "passed" or saved["source_sha"] == result["source_sha"]:
                    saved["latest_passing"] = latest
                    continue
                row["baseline_update"] = "promoted"
            active[key] = {
                "configuration": config["id"], "workload": row["workload"], "metric": metric,
                "contract_sha256": contract, **latest, "latest_passing": latest,
            }
    if regression:
        result["outcome"] = "failed"
    return result, [active[key] for key in sorted(active)]
