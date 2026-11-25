#!/usr/bin/env python3
"""Collect completed failing job logs for a bounded upgrade repair attempt."""

import argparse
import json
import os
from pathlib import Path

from lib.upgrade_resolution import gh, REPO


def collect(output):
    run = os.environ["GITHUB_RUN_ID"]
    if not run.isdecimal():
        raise ValueError("invalid workflow run ID")
    output.mkdir(parents=True, exist_ok=True)
    pages = gh("api", "--paginate", "--slurp", f"repos/{REPO}/actions/runs/{run}/jobs?per_page=100")
    for page in json.loads(pages):
        for job in page["jobs"]:
            if job["conclusion"] == "failure":
                identifier = job["id"]
                if type(identifier) is not int or identifier <= 0:
                    raise ValueError("invalid workflow job ID")
                (output / f"actions-{identifier}.log").write_text(
                    gh("api", f"repos/{REPO}/actions/jobs/{identifier}/logs"), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    collect(parser.parse_args().output)
