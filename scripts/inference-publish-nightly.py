#!/usr/bin/env python3
"""Publish aggregate nightly inference evidence and saved baselines to Hugging Face."""

import argparse
import os
from pathlib import Path
import re
import sys
from tempfile import TemporaryDirectory

from lib.ci_artifacts import read_json, write_json
from lib.inference_baselines import apply_baselines, SHA
from lib.inference_markdown import dataset_card, dataset_charts, run_summary, utc
from lib.inference_nightly_results import nightly_run
from lib.inference_parquet import read_run, write_run
from lib.inference_png import export_charts


REPO = "meshllm/skippy-inference-qualification"
SOURCE_REPO = "Mesh-LLM/skippy"
HISTORY_LIMIT = 64
RUN_PATH = re.compile(r"runs/([1-9][0-9]*)/([1-9][0-9]*)\.parquet")
CHART_PATH = re.compile(r"charts/qualification-[1-4]\.png")


def admit(environment):
    expected = {
        "GITHUB_EVENT_NAME": "schedule", "GITHUB_REPOSITORY": SOURCE_REPO,
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_WORKFLOW_REF": f"{SOURCE_REPO}/.github/workflows/inference.yml@refs/heads/main",
        "GITHUB_WORKFLOW_SHA": environment.get("GITHUB_SHA"),
    }
    if any(environment.get(key) != value for key, value in expected.items()):
        raise ValueError("only scheduled qualification on protected main can publish")
    source = environment["GITHUB_SHA"]
    if not SHA.fullmatch(source) or environment.get("SOURCE_SHA") != source:
        raise ValueError("nightly publication source must match the scheduled SHA")
    run_id, attempt, number = (environment[key] for key in (
        "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_RUN_NUMBER",
    ))
    if any(not re.fullmatch(r"[1-9][0-9]*", value) for value in (run_id, attempt, number)):
        raise ValueError("invalid nightly run identity")
    started_at = environment["RUN_STARTED_AT"]
    utc(started_at)
    result = environment["MATRIX_RESULT"]
    if result not in {"success", "failure", "cancelled"}:
        raise ValueError("invalid nightly matrix result")
    return source, run_id, attempt, int(number), started_at, result == "success"


def prior_state(files, fetch):
    if "baselines.json" not in files:
        if any(path.startswith("runs/") for path in files):
            raise ValueError("nightly history exists without a baseline manifest")
        return {"schema_version": 1, "last_run_number": 0, "history": [], "baselines": []}
    state = read_json(fetch("baselines.json"))
    history = state["history"]
    if (type(state["schema_version"]) is not int or state["schema_version"] != 1
            or type(state["last_run_number"]) is not int or state["last_run_number"] < 1
            or not 0 < len(history) <= HISTORY_LIMIT or len(set(history)) != len(history)
            or any(not RUN_PATH.fullmatch(path) or path not in files for path in history)
            or type(state["baselines"]) is not list):
        raise ValueError("invalid nightly baseline manifest")
    return state


def load_history(paths, fetch):
    history = []
    for path in paths:
        run = read_run(fetch(path))
        expected = "-".join(RUN_PATH.fullmatch(path).groups())
        if run["id"] != expected or run["profile"] != "nightly" or not SHA.fullmatch(run["source_sha"]):
            raise ValueError("published inference history identity mismatch")
        history.append(run)
    return history


def publish(api, fetch_file, add, delete, evidence, identity):
    source, run_id, attempt, number, started_at, matrix_passed = identity
    info = api.dataset_info(REPO, token=False)
    if info.id != REPO or info.private is not False or info.gated is not False or not SHA.fullmatch(info.sha):
        raise ValueError("expected the public, ungated qualification dataset")
    files = {entry.rfilename for entry in info.siblings}
    fetch = lambda path: Path(fetch_file(REPO, path, repo_type="dataset", revision=info.sha, token=False))
    state = prior_state(files, fetch)
    path = f"runs/{run_id}/{attempt}.parquet"
    if path in files:
        if path not in state["history"]:
            raise ValueError("run already exists outside the active publication history")
        run = load_history([path], fetch)[0]
        if run["source_sha"] != source:
            raise ValueError("published run source mismatch")
        return run
    if state["history"]:
        previous_id, previous_attempt = map(int, RUN_PATH.fullmatch(state["history"][0]).groups())
        if (number < state["last_run_number"]
                or (number == state["last_run_number"]
                    and (int(run_id) != previous_id or int(attempt) <= previous_attempt))):
            raise ValueError("older nightly qualification cannot replace newer publication")

    run = nightly_run(evidence, source, run_id, attempt, started_at, matrix_passed)
    if state["history"]:
        run["baseline_revision"] = info.sha
    run, baselines = apply_baselines(run, state["baselines"])
    history_paths = state["history"][:HISTORY_LIMIT - 1]
    history = load_history(history_paths, fetch)
    report = {"schema_version": 1, "runs": [run, *history], "active_baselines": baselines}
    manifest = {"schema_version": 1, "last_run_number": number,
                "history": [path, *history_paths], "baselines": baselines}
    with TemporaryDirectory(prefix="skippy-qualification-") as staging:
        output = Path(staging)
        parquet = output / path
        parquet.parent.mkdir(parents=True)
        write_run(parquet, run)
        write_json(output / "baselines.json", manifest)
        charts = dataset_charts(report)
        export_charts(charts, output)
        (output / "README.md").write_text(dataset_card(report, charts), encoding="utf-8")
        staged = {file.relative_to(output).as_posix(): file for file in output.rglob("*") if file.is_file()}
        operations = [add(path_in_repo=name, path_or_fileobj=file) for name, file in sorted(staged.items())]
        operations.extend(delete(path_in_repo=name) for name in sorted(files - staged.keys())
                          if CHART_PATH.fullmatch(name))
        receipt = api.create_commit(
            repo_id=REPO, repo_type="dataset", operations=operations, parent_commit=info.sha,
            commit_message=f"Publish Skippy nightly inference qualification {run['id']}",
        )
        print(f"Published {receipt.commit_url}", flush=True)
    return run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    identity = admit(os.environ)
    if not os.environ.get("HF_TOKEN"):
        raise ValueError("HF_TOKEN is required for nightly publication")
    from huggingface_hub import CommitOperationAdd, CommitOperationDelete, HfApi, hf_hub_download

    run = publish(HfApi(), hf_hub_download, CommitOperationAdd, CommitOperationDelete, args.evidence, identity)
    report = {"schema_version": 1, "runs": [run]}
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as stream:
        stream.write(run_summary(report, run["id"]))
    return 0 if run["outcome"] == "passed" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as error:
        sys.exit(f"Nightly inference publication failed: {error}")
