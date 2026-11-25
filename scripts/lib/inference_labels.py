"""Display labels for qualification records and charts."""

from functools import cache

from .ci_artifacts import read_json
from .inference_ladder import LADDER


@cache
def model_names():
    return {
        model["id"]: model["model"].rsplit("/", 1)[-1].removesuffix("-Instruct")
        for model in read_json(LADDER)["models"]
    }


def configuration_label(configuration):
    model, _, placement = configuration.rpartition("-")
    count = {"standalone": "1 Spark", "split": "2 Sparks"}.get(placement)
    name = model_names().get(model)
    return f"{name} ({count})" if name and count else configuration


def workload_label(workload):
    return {
        "DGX Spark AgentX nightly v1": "Nightly AgentX (v1)",
        "DGX Spark smoke v1": "Smoke test (v1)",
        "DGX Spark smoke v1 (experimental split)": "Smoke test (v1, experimental split)",
    }.get(workload, workload)
