#!/usr/bin/env python3
"""Render normalized inference qualification JSON as a dataset card or Actions summary."""

import argparse
from pathlib import Path
import sys

from lib.ci_artifacts import read_json
from lib.inference_markdown import dataset_card, dataset_charts, run_summary
from lib.inference_png import export_charts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("format", choices=("dataset-card", "summary"))
    parser.add_argument("input", type=Path, help="Version 1 qualification report JSON")
    parser.add_argument("--run", help="Run ID to render in the Actions summary")
    parser.add_argument("--output", type=Path,
                        help="Write Markdown to this path and export dataset PNGs beside it")
    args = parser.parse_args()
    if (args.format == "summary") != (args.run is not None):
        parser.error("--run is required for summary and only applies to that format")
    try:
        report = read_json(args.input)
        if args.run is not None:
            markdown = run_summary(report, args.run)
        else:
            charts = dataset_charts(report)
            markdown = dataset_card(report, charts)
            if args.output:
                export_charts(charts, args.output.parent)
        if args.output:
            args.output.write_text(markdown, encoding="utf-8")
        else:
            sys.stdout.write(markdown)
    except (KeyError, TypeError, ValueError, OSError) as error:
        parser.exit(1, f"Cannot render qualification report: {error}\n")


if __name__ == "__main__":
    main()
