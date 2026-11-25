#!/usr/bin/env python3
"""Dispatch source-pinned inference qualification and retain native job evidence."""

import argparse
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from lib.ci_artifacts import read_json, write_json
from lib.ci_process import cancel_on_signal
from lib.inference_ladder import plan
from lib.inference_nightly_metrics import collect_nightly
from lib.inference_throughput import collect
from lib.jy_qualification import Pilot, placement

TERMINAL = {"succeeded", "failed", "cancelled", "expired"}
FAILURE = TERMINAL - {"succeeded"}


def resource_id(identifier):
    if not isinstance(identifier, str) or not re.fullmatch(r"[a-zA-Z0-9_]+", identifier):
        raise ValueError("invalid JY resource id")
    return identifier


def cancel_path(pilot, identifier):
    resource_id(identifier)
    if identifier.startswith("frm_"):
        formation = identifier
    else:
        job = pilot.api("GET", f"/v1/jobs/{identifier}")
        formation = job.get("formation_id")
        if formation is None:
            return None if job.get("status") in TERMINAL else f"/v1/jobs/{identifier}/cancel"
    formation = resource_id(formation)
    group = pilot.api("GET", f"/v1/formations/{formation}")
    members = group.get("members")
    return (None if members and all(member.get("status") in TERMINAL for member in members)
            else f"/v1/formations/{formation}/cancel")


def cancel(pilot, ledger):
    errors = []
    for identifier in read_json(ledger):
        try:
            path = cancel_path(pilot, identifier)
            if path:
                pilot.api("POST", path)
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
            if "already_terminal" not in str(error):
                errors.append(str(error))
    if errors:
        raise RuntimeError("\n".join(errors))


def launch(pilot, path, body, deadline, split):
    attempt = 0
    while time.monotonic() < deadline:
        if split:
            inventory = pilot.api("GET", "/v1/agents")
            try:
                leader, worker = placement(inventory, os.environ.get("RUNNER_NAME", ""),
                                           attempt, body["ref"])
            except ValueError as error:
                print(f"Waiting for Sparks: {error}", flush=True)
                time.sleep(15)
                continue
            body["inputs"].update(leader_node=leader, worker_node=worker)
            attempt += 1
        try:
            return pilot.api("POST", path, body)
        except RuntimeError as error:
            if "role_unplaceable" not in str(error):
                raise
            print(f"Waiting for an atomic Spark pair: {error}", flush=True)
            time.sleep(15)
    raise RuntimeError("JY qualification timed out waiting for placement")


def wait(pilot, handle, row, sha, output, deadline):
    if handle.startswith("frm_"):
        formation = pilot.api("GET", f"/v1/formations/{handle}")
        leader = next((member for member in formation["members"] if member.get("role") == "leader"), {})
        handle = resource_id(leader.get("job_id"))
    while time.monotonic() < deadline:
        job = pilot.api("GET", f"/v1/jobs/{handle}")
        write_json(output / f"{row}-job.json", job)
        status = job["status"]
        if status in FAILURE:
            raise RuntimeError(f"{row} failed: {status}")
        if status == "succeeded":
            if job["repo_rev"] != sha:
                raise ValueError("JY result source SHA mismatch")
            if job.get("formation_id"):
                formation_id = resource_id(job["formation_id"])
                formation = pilot.api("GET", f"/v1/formations/{formation_id}")
                write_json(output / f"{row}-formation.json", formation)
                if formation.get("status") in FAILURE:
                    raise RuntimeError("split formation failed")
                if formation.get("status") != "succeeded":
                    time.sleep(5)
                    continue
            return job
        time.sleep(15)
    raise RuntimeError("JY qualification timed out")


def record(ledger, identifiers):
    with tempfile.NamedTemporaryFile(dir=ledger.parent, delete=False) as handle:
        temporary = Path(handle.name)
    try:
        write_json(temporary, identifiers)
        temporary.replace(ledger)
    finally:
        temporary.unlink(missing_ok=True)


def dispatch(pilot, profile, sha, output, configuration, collect_public=False):
    result = plan(profile, sha)
    if configuration is not None:
        result["rows"] = [row for row in result["rows"] if row["id"] == configuration]
        if len(result["rows"]) != 1:
            raise ValueError("configuration not in qualification plan")
    output.mkdir(parents=True, exist_ok=True)
    write_json(output / "plan.json", result)
    ledger = output / "jobs.json"
    identifiers = []
    record(ledger, identifiers)
    # Manual qualification may queue behind another PR's atomic Spark pair;
    # a PR profile waits for placement instead of failing fast, and the job
    # timeout in inference.yml carries the same headroom.
    deadline = time.monotonic() + (5400 if profile == "pr" else 21000)
    try:
        for row in result["rows"]:
            identity, mode = row["id"], row["mode"]
            model_key = identity.removesuffix(f"-{mode}")
            template = "inference-" + ("pr-" if profile == "pr" else "") + mode
            run_id = os.environ.get("GITHUB_RUN_ID", "local")
            attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
            body = {
                "ref": sha,
                "inputs": {"profile": profile, "model": row["model"].split("/")[1],
                           "model_key": model_key},
                "idempotency_key": f"inference-{run_id}-{profile}-{attempt}-{identity}-{sha}",
            }
            launched = launch(pilot, f"/v1/repos/skippy/templates/{template}/launch", body,
                              deadline, mode == "split")
            handle = resource_id(launched.get("job_id"))
            identifiers.append(handle)
            record(ledger, identifiers)
            write_json(output / f"{identity}-launch.json", launched)
            job = wait(pilot, handle, identity, sha, output, deadline)
            if collect_public:
                collect_nightly(pilot, job, row, sha, output, model_key, profile)
            elif profile == "pr":
                try:
                    collect(pilot, job, row, sha, output, model_key)
                except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
                    print(f"{identity} smoke throughput unavailable: {error}", file=sys.stderr)
    except (Exception, KeyboardInterrupt):
        try:
            cancel(pilot, ledger)
        except Exception as error:
            print(f"JY cleanup failed: {error}", file=sys.stderr)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    action = actions.add_parser("dispatch")
    action.add_argument("profile")
    action.add_argument("sha")
    action.add_argument("output", type=Path)
    action.add_argument("configuration", nargs="?")
    action.add_argument("--collect-public", action="store_true")
    actions.add_parser("cancel").add_argument("ledger", type=Path)
    actions.add_parser("control-preflight")
    args = parser.parse_args()
    pilot = Pilot()
    if args.action == "dispatch":
        dispatch(pilot, args.profile, args.sha, args.output, args.configuration, args.collect_public)
    elif args.action == "cancel":
        cancel(pilot, args.ledger)
    else:
        result = pilot.api("GET", "/v1/repos/skippy")
        print(f'JY repository access verified: {result["slug"]}')


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
