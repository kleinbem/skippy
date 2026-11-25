#!/usr/bin/env python3
"""Reject existing graph-contract regressions and report uncovered upstream models."""

import argparse
import json
import sys
from pathlib import Path

from lib.graph_contracts import check


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = check(args.root, args.source_root, json.loads(args.report.read_text()), args.output)
    print(f'checked {result["registry_cases"]} registered and {result["executable_cases"]} total synthetic graph cases')


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, IndexError) as error:
        sys.exit(str(error))
